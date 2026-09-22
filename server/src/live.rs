//! Live updates over server-sent events.
//!
//! Every question owns a broadcast channel that is pinged whenever somebody
//! answers or the question is closed. A ping makes the stream send a fresh
//! snapshot, so a slow or reconnecting client always gets the current state
//! instead of a backlog of stale ones.

use std::convert::Infallible;

use axum::response::sse::{Event, KeepAlive, Sse};
use futures_core::Stream;
use tokio::sync::broadcast::error::RecvError;

use crate::state::AppState;

/// Streams the state of a question until the client goes away.
pub fn stream(
    state: AppState,
    id: String,
) -> Sse<impl Stream<Item = Result<Event, Infallible>> + use<>> {
    let stream = async_stream::stream! {
        let Some(mut updates) = subscribe(&state, &id).await else {
            // The question is gone; the browser will notice on its next reload.
            return;
        };

        if let Some(event) = snapshot(&state, &id).await {
            yield Ok(event);
        }

        loop {
            match updates.recv().await {
                Ok(()) => match snapshot(&state, &id).await {
                    Some(event) => yield Ok(event),
                    None => break,
                },
                // Only the newest state matters, so dropped pings are fine.
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn subscribe(state: &AppState, id: &str) -> Option<tokio::sync::broadcast::Receiver<()>> {
    Some(state.store.read().await.get(id)?.updates.subscribe())
}

async fn snapshot(state: &AppState, id: &str) -> Option<Event> {
    let store = state.store.read().await;
    let question = store.get(id)?;
    Event::default()
        .event("state")
        .json_data(question.live_state())
        .inspect_err(|error| tracing::error!(%error, "serializing the live state failed"))
        .ok()
}
