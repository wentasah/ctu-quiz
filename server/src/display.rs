//! The presentation view that the teacher shows on a projector.
//!
//! It requires the same OIDC login as the students' pages and shows the room
//! of whoever is logged in, so its URL is the same for every teacher and
//! nobody else can watch or close their questions there.

use anyhow::{Context, Result};
use axum::{
    extract::{Form, State},
    response::{Html, IntoResponse, Redirect, Response},
};
use chrono::Utc;
use qrcode::{EcLevel, QrCode, render::svg};
use serde::Deserialize;

use crate::{
    auth::User,
    error::PageError,
    live,
    state::{AppState, Store},
    views::{self, Countdown, DisplayPage, bars, page_title},
};
use askama::Template;

/// `GET /d` - question, QR code, code to type, live answer count and, once
/// the question is closed, the bar graph (requirements 9, 11 and 12).
///
/// It shows whichever question is being asked in the room of the teacher
/// logged in, so the window stays open for the whole lecture.
pub async fn display(
    State(state): State<AppState>,
    User(user): User,
) -> Result<Response, PageError> {
    let mut store = state.store.write().await;
    let room = room_token(&store, &user.login)?;
    let id = store
        .current_question(&room)
        .map(|question| question.id.clone())
        .ok_or_else(not_found)?;

    // The countdown of `--close-after` starts once the question is on the
    // screen, not when it was asked: the teacher may have had to log in here
    // first.
    let question = store.get_mut(&id).ok_or_else(not_found)?;
    if let Some(at) = question.start_countdown(Utc::now()) {
        tracing::info!(question = %id, closes_at = %at, "countdown started");
        state.close_at(id.clone(), at);
    }
    let question = store.get(&id).ok_or_else(not_found)?;

    let page = DisplayPage {
        prefix: views::prefix(),
        show_answers: true,
        question: question.id.clone(),
        title: page_title(Some(question)),
        prompt_html: question.content.prompt_html.clone(),
        open: question.is_open(),
        revealed: question.revealed,
        scored: question.content.is_scored(),
        answer_count: question.responses.len(),
        code: question.code.clone(),
        answer_url: question.answer_url.clone(),
        host: host_hint(&state),
        qr_svg: qr_svg(&question.answer_url)?,
        bars: bars(question, None),
        countdown: Countdown::of(question, Utc::now()),
    };

    Ok(Html(page.render()?).into_response())
}

#[derive(Debug, Deserialize)]
pub struct QuestionForm {
    /// Id of the question the page was showing when the button was pressed.
    question: String,
}

/// `POST /d/close` - closes the question from the projector, as the teacher
/// would from the CLI. A waiting `quiz ask` notices it on its event stream
/// and saves the answers.
pub async fn close(
    State(state): State<AppState>,
    User(user): User,
    Form(form): Form<QuestionForm>,
) -> Result<Response, PageError> {
    {
        let mut store = state.store.write().await;
        let room = room_token(&store, &user.login)?;

        // The teacher may have asked the next question in the meantime;
        // that one was not on the screen, so it stays open.
        if store.room(&room).and_then(|room| room.current.as_deref()) == Some(&form.question) {
            let question = store.get_mut(&form.question).ok_or_else(not_found)?;
            if question.close() {
                tracing::info!(
                    question = %form.question,
                    teacher = %user.login,
                    answers = question.responses.len(),
                    counts = ?question.counts(),
                    "question closed in the presentation view"
                );
            }
            store.notify_room_of(&form.question);
        }
    }

    // Redirect so that reloading the page does not post the form again.
    Ok(Redirect::to(&views::link("/d")).into_response())
}

/// `POST /d/reveal` - marks the right answers in the results, as `quiz
/// reveal` does. A `quiz ask` waiting for Enter to do so notices and ends.
pub async fn reveal(
    State(state): State<AppState>,
    User(user): User,
    Form(form): Form<QuestionForm>,
) -> Result<Response, PageError> {
    {
        let mut store = state.store.write().await;
        let room = room_token(&store, &user.login)?;

        // Only the question on the screen; any other one was not seen.
        if store.room(&room).and_then(|room| room.current.as_deref()) == Some(&form.question) {
            let question = store.get_mut(&form.question).ok_or_else(not_found)?;
            if question.reveal() {
                tracing::info!(
                    question = %form.question,
                    teacher = %user.login,
                    "answers revealed in the presentation view"
                );
            }
            store.notify_room_of(&form.question);
        }
    }

    Ok(Redirect::to(&views::link("/d")).into_response())
}

/// `GET /d/events` - live updates for the presentation view.
pub async fn events(
    State(state): State<AppState>,
    User(user): User,
) -> Result<Response, PageError> {
    let room = room_token(&*state.store.read().await, &user.login)?;
    Ok(live::stream(state, live::Source::Presentation(room)).into_response())
}

/// Token of the room of `login`. Anybody else, students included, has no
/// room and so nothing to present.
fn room_token(store: &Store, login: &str) -> Result<String, PageError> {
    store
        .room_of_teacher(login)
        .map(|room| room.token.clone())
        .ok_or_else(|| {
            PageError::not_found(
                "Nemáte otevřenou žádnou místnost. Otázku položíte příkazem quiz ask.",
            )
        })
}

/// What students have to type into a browser to reach the main page.
fn host_hint(state: &AppState) -> String {
    let url = &state.cfg.public_url;
    let host = url.host_str().unwrap_or("");
    let port = match url.port() {
        Some(port) => format!(":{port}"),
        None => String::new(),
    };
    format!("{host}{port}{}", url.path().trim_end_matches('/'))
}

/// Renders the answer URL as an SVG QR code, embedded into the page.
fn qr_svg(url: &str) -> Result<String> {
    let code = QrCode::with_error_correction_level(url, EcLevel::M)
        .with_context(|| format!("{url} does not fit into a QR code"))?;
    let svg = code
        .render::<svg::Color>()
        .min_dimensions(320, 320)
        .quiet_zone(true)
        .build();
    // The renderer emits an XML declaration, which has no place inside HTML.
    Ok(match svg.split_once("?>") {
        Some((declaration, rest)) if declaration.starts_with("<?xml") => rest.to_owned(),
        _ => svg,
    })
}

fn not_found() -> PageError {
    PageError::not_found("Tato otázka už na serveru není.")
}

#[cfg(test)]
mod tests {
    use super::qr_svg;

    #[test]
    fn renders_a_qr_code() {
        let svg = qr_svg("https://osy.fel.cvut.cz/q/0123456789abcdef").unwrap();
        assert!(
            svg.starts_with("<svg"),
            "got: {}",
            &svg[..40.min(svg.len())]
        );
        assert!(svg.contains("</svg>"));
    }
}
