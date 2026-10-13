//! Live updates over server-sent events.
//!
//! Every question and every room owns a broadcast channel that is pinged
//! whenever somebody answers, the question is closed or the teacher asks a
//! new one. A ping makes the stream send a fresh snapshot, so a slow or
//! reconnecting client always gets the current state instead of a backlog of
//! stale ones.
//!
//! Both the presentation view and the students follow the room, which is what
//! lets their pages move on to the next question by themselves.

use std::{convert::Infallible, sync::Arc};

use axum::response::sse::{Event, KeepAlive, Sse};
use futures_core::Stream;
use tokio::sync::broadcast::{self, error::RecvError};

use crate::state::AppState;

/// What a stream follows.
#[derive(Debug, Clone)]
pub enum Source {
    /// A room, for the students in it: the state of whatever question is
    /// being asked there, and of the next one after that.
    Room(String),
    /// The same for the presentation view of a room, which also counts as
    /// open for as long as the stream runs.
    Presentation(String),
}

impl Source {
    fn room(&self) -> &str {
        match self {
            Source::Room(token) | Source::Presentation(token) => token,
        }
    }
}

/// Streams the state of a question or of a room until the client goes away.
pub fn stream(
    state: AppState,
    source: Source,
) -> Sse<impl Stream<Item = Result<Event, Infallible>> + use<>> {
    let stream = async_stream::stream! {
        // `_presenting` is dropped together with the stream, which happens
        // once the browser window is closed.
        let Some((mut updates, _presenting)) = subscribe(&state, &source).await else {
            yield Ok(gone());
            return;
        };

        if let Some(event) = snapshot(&state, &source).await {
            yield Ok(event);
        }

        loop {
            match updates.recv().await {
                Ok(()) => match snapshot(&state, &source).await {
                    Some(event) => yield Ok(event),
                    // The question or the whole room has been dropped, for
                    // instance because the teacher logged out.
                    None => {
                        yield Ok(gone());
                        break;
                    }
                },
                // Only the newest state matters, so dropped pings are fine.
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => {
                    yield Ok(gone());
                    break;
                }
            }
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Tells the page that what it was showing is no longer on the server, so
/// that it reloads and says so instead of waiting for updates forever.
fn gone() -> Event {
    Event::default().event("gone").data("1")
}

async fn subscribe(
    state: &AppState,
    source: &Source,
) -> Option<(broadcast::Receiver<()>, Option<Arc<()>>)> {
    let store = state.store.read().await;
    let room = store.room(source.room())?;
    let presenting = match source {
        Source::Room(_) => None,
        Source::Presentation(_) => Some(room.presenters.clone()),
    };
    Some((room.updates.subscribe(), presenting))
}

async fn snapshot(state: &AppState, source: &Source) -> Option<Event> {
    let store = state.store.read().await;
    let live = store.room_state(source.room())?;
    Event::default()
        .event("state")
        .json_data(live)
        .inspect_err(|error| tracing::error!(%error, "serializing the live state failed"))
        .ok()
}
