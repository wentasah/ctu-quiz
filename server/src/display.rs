//! The presentation view that the teacher shows on a projector.
//!
//! Its URL contains an unguessable token, which is what authorizes access: the
//! CLI receives the URL over the authenticated API and opens it in a browser,
//! so that nothing has to be logged into on the presenting machine. Students
//! never see this URL - they get the one from the QR code.

use anyhow::{Context, Result};
use axum::{
    extract::{Form, Path, State},
    response::{Html, IntoResponse, Redirect, Response},
};
use qrcode::{EcLevel, QrCode, render::svg};
use serde::Deserialize;

use crate::{
    error::PageError,
    live,
    state::AppState,
    views::{self, DisplayPage, bars, page_title},
};
use askama::Template;

/// `GET /d/{token}` - question, QR code, code to type, live answer count and,
/// once the question is closed, the bar graph (requirements 9, 11 and 12).
///
/// The URL belongs to the teacher's room and shows whichever question is being
/// asked there, so the window stays open for the whole lecture.
pub async fn display(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Response, PageError> {
    let store = state.store.read().await;
    let room = store.room_by_display_token(&token).ok_or_else(not_found)?;
    let question = store.current_question(&room.token).ok_or_else(not_found)?;

    let page = DisplayPage {
        prefix: views::prefix(),
        show_answers: true,
        token: token.clone(),
        question: question.id.clone(),
        title: page_title(Some(question)),
        prompt_html: question.content.prompt_html.clone(),
        open: question.is_open(),
        revealed: question.revealed,
        answer_count: question.responses.len(),
        code: question.code.clone(),
        answer_url: question.answer_url.clone(),
        host: host_hint(&state),
        qr_svg: qr_svg(&question.answer_url)?,
        bars: bars(question, None),
    };

    Ok(Html(page.render()?).into_response())
}

#[derive(Debug, Deserialize)]
pub struct CloseForm {
    /// Id of the question the page was showing when the button was pressed.
    question: String,
}

/// `POST /d/{token}/close` - closes the question from the projector, as the
/// teacher would from the CLI. A waiting `quiz ask` notices it on its event
/// stream and saves the answers.
pub async fn close(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Form(form): Form<CloseForm>,
) -> Result<Response, PageError> {
    {
        let mut store = state.store.write().await;
        let room = store.room_by_display_token(&token).ok_or_else(not_found)?;

        // The teacher may have asked the next question in the meantime;
        // that one was not on the screen, so it stays open.
        if room.current.as_deref() == Some(form.question.as_str()) {
            let teacher = room.teacher.clone();
            let question = store.get_mut(&form.question).ok_or_else(not_found)?;
            if question.close() {
                tracing::info!(
                    question = %form.question,
                    %teacher,
                    answers = question.responses.len(),
                    counts = ?question.counts(),
                    "question closed in the presentation view"
                );
            }
            store.notify_room_of(&form.question);
        }
    }

    // Redirect so that reloading the page does not post the form again.
    Ok(Redirect::to(&views::link(&format!("/d/{token}"))).into_response())
}

/// `GET /d/{token}/events` - live updates for the presentation view.
pub async fn events(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Response, PageError> {
    let room = state
        .store
        .read()
        .await
        .room_by_display_token(&token)
        .map(|room| room.token.clone())
        .ok_or_else(not_found)?;

    Ok(live::stream(state, live::Source::Presentation(room)).into_response())
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
