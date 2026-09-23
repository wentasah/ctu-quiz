//! The presentation view that the teacher shows on a projector.
//!
//! Its URL contains an unguessable token, which is what authorizes access: the
//! CLI receives the URL over the authenticated API and opens it in a browser,
//! so that nothing has to be logged into on the presenting machine. Students
//! never see this URL - they get the one from the QR code.

use anyhow::{Context, Result};
use axum::{
    extract::{Path, State},
    response::{Html, IntoResponse, Response},
};
use qrcode::{EcLevel, QrCode, render::svg};

use crate::{
    error::PageError,
    live,
    state::AppState,
    views::{self, DisplayPage, bars},
};
use askama::Template;

/// `GET /d/{token}` - question, QR code, code to type, live answer count and,
/// once the question is closed, the bar graph (requirements 9, 11 and 12).
pub async fn display(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Response, PageError> {
    let store = state.store.read().await;
    let question = store.by_display_token(&token).ok_or_else(not_found)?;

    let page = DisplayPage {
        prefix: views::prefix(),
        show_answers: true,
        token: token.clone(),
        question: question.id.clone(),
        title: question.title().to_owned(),
        prompt_html: question.content.prompt_html.clone(),
        open: question.is_open(),
        answer_count: question.responses.len(),
        code: question.code.clone(),
        answer_url: question.answer_url.clone(),
        host: host_hint(&state),
        qr_svg: qr_svg(&question.answer_url)?,
        bars: bars(question, None),
    };

    Ok(Html(page.render()?).into_response())
}

/// `GET /d/{token}/events` - live updates for the presentation view.
pub async fn events(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Response, PageError> {
    let id = state
        .store
        .read()
        .await
        .by_display_token(&token)
        .map(|question| question.id.clone())
        .ok_or_else(not_found)?;

    Ok(live::stream(state, live::Source::Question(id)).into_response())
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
