//! Live updates over server-sent events.
//!
//! Every question and every room owns a broadcast channel that is pinged
//! whenever somebody answers, the question is closed or the teacher asks a
//! new one. A ping makes the stream send a fresh snapshot, so a slow or
//! reconnecting client always gets the current state instead of a backlog of
//! stale ones.
//!
//! The presentation view follows one question, the students follow the room,
//! which is what lets their page move on to the next question by itself.

use std::convert::Infallible;

use axum::response::sse::{Event, KeepAlive, Sse};
use futures_core::Stream;
use tokio::sync::broadcast::error::RecvError;

use crate::state::AppState;

/// What a stream follows.
#[derive(Debug, Clone)]
pub enum Source {
    /// One question, for the presentation view.
    Question(String),
    /// A room, for the students in it: the state of whatever question is
    /// being asked there, and of the next one after that.
    Room(String),
}

/// Streams the state of a question or of a room until the client goes away.
pub fn stream(
    state: AppState,
    source: Source,
) -> Sse<impl Stream<Item = Result<Event, Infallible>> + use<>> {
    let stream = async_stream::stream! {
        let Some(mut updates) = subscribe(&state, &source).await else {
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
) -> Option<tokio::sync::broadcast::Receiver<()>> {
    let store = state.store.read().await;
    Some(match source {
        Source::Question(id) => store.get(id)?.updates.subscribe(),
        Source::Room(token) => store.room(token)?.updates.subscribe(),
    })
}

async fn snapshot(state: &AppState, source: &Source) -> Option<Event> {
    let store = state.store.read().await;
    let live = match source {
        Source::Question(id) => store.get(id)?.live_state(),
        Source::Room(token) => store.room_state(token)?,
    };
    Event::default()
        .event("state")
        .json_data(live)
        .inspect_err(|error| tracing::error!(%error, "serializing the live state failed"))
        .ok()
}
