//! The pages students use: the main page, and the room they stay in.
//!
//! Students reach a room once, by scanning the QR code or by typing the
//! numeric code, and stay on it: every question the teacher asks afterwards
//! appears on the page they already have open (the event stream tells the page
//! to re-render itself).

use axum::{
    extract::{Form, Path, State},
    response::{Html, IntoResponse, Redirect, Response},
};
use chrono::Utc;
use serde::Deserialize;

use crate::{
    auth::User,
    error::PageError,
    i18n::Lang,
    live,
    state::{AppState, Response as Answer},
    views::{self, AnswerPage, IndexPage, Verdict, bars, page_title},
};
use askama::Template;

/// `GET /` - asks for the question code (requirement 10).
pub async fn index(User(user): User, lang: Lang) -> Result<Response, PageError> {
    Ok(Html(code_page(user, lang, String::new())?).into_response())
}

#[derive(Debug, Deserialize)]
pub struct JoinForm {
    code: String,
}

/// `POST /join` - sends the student to the URL the QR code points to.
pub async fn join(
    State(state): State<AppState>,
    User(user): User,
    lang: Lang,
    Form(form): Form<JoinForm>,
) -> Result<Response, PageError> {
    // Be forgiving about spaces and dashes in a code read off a projector.
    let code: String = form.code.chars().filter(char::is_ascii_digit).collect();

    let Some(room) = state
        .store
        .read()
        .await
        .room_by_code(&code)
        .map(|room| room.token.clone())
    else {
        tracing::info!(login = %user.login, %code, "unknown question code");
        let error = lang
            .msg("unknown-code")
            .arg("code", code.as_str())
            .to_string();
        return Ok(Html(code_page(user, lang, error)?).into_response());
    };

    tracing::info!(login = %user.login, %code, %room, "student entered a code");
    Ok(Redirect::to(&views::link(&format!("/r/{room}"))).into_response())
}

/// `GET /r/{token}` - the page a student stays on for the whole lecture: the
/// question that is being asked, its results once it is closed, or a note
/// that the next question is yet to come.
pub async fn room(
    State(state): State<AppState>,
    User(user): User,
    lang: Lang,
    Path(token): Path<String>,
) -> Result<Response, PageError> {
    let store = state.store.read().await;
    let room = store.room(&token).ok_or_else(not_found)?;
    let current = store.current_question(&token);

    let mut page = AnswerPage {
        prefix: views::prefix(),
        lang,
        user,
        room: room.token.clone(),
        code: room.code.clone(),
        question: String::new(),
        title: page_title(current, lang),
        prompt_html: String::new(),
        show_question: true,
        show_answers: true,
        waiting: true,
        open: false,
        revealed: false,
        answer_count: 0,
        chosen_label: String::new(),
        verdict: None,
        bars: Vec::new(),
    };

    if let Some((question, content)) =
        current.and_then(|question| Some((question, question.content.as_ref()?)))
    {
        let chosen = question.responses.get(&page.user.login).map(|r| r.index);
        page.question = question.id.clone();
        page.show_question = question.view.show_question;
        page.show_answers = question.view.show_answers;
        if page.show_question {
            page.prompt_html = content.prompt_html.clone();
        }
        page.waiting = false;
        page.open = question.is_open();
        page.revealed = question.revealed;
        page.answer_count = question.responses.len();
        page.chosen_label = chosen
            .and_then(|index| content.answers.get(index))
            .map(|answer| answer.label.clone())
            .unwrap_or_default();
        if question.revealed {
            page.verdict = chosen.and_then(|index| Verdict::of(content, index));
        }
        page.bars = bars(question, chosen);
    }

    Ok(Html(page.render()?).into_response())
}

#[derive(Debug, Deserialize)]
pub struct AnswerForm {
    /// Id of the question the page was showing when the student answered.
    question: String,
    answer: usize,
}

/// `POST /r/{token}/answer` - records one answer.
pub async fn answer(
    State(state): State<AppState>,
    User(user): User,
    Path(token): Path<String>,
    Form(form): Form<AnswerForm>,
) -> Result<Response, PageError> {
    {
        let mut store = state.store.write().await;
        let room = store.room(&token).ok_or_else(not_found)?;

        // The teacher may have moved on while the page was being read; the
        // answer belongs to the question the student saw, so it is dropped
        // and the page is re-rendered with the question that is current now.
        if room.current.as_deref() != Some(form.question.as_str()) {
            tracing::info!(
                login = %user.login,
                %room.token,
                question = %form.question,
                "answer to a question that is no longer being asked"
            );
            return Ok(Redirect::to(&views::link(&format!("/r/{token}"))).into_response());
        }

        let id = form.question.clone();
        let question = store.get_mut(&id).ok_or_else(not_found)?;

        if !question.is_open() {
            tracing::info!(question = %id, login = %user.login, "answer to a closed question");
            return Err(PageError::new(
                axum::http::StatusCode::CONFLICT,
                "question-is-closed",
                "question-not-accepting",
            ));
        }

        let Some(answer) = question
            .content
            .as_ref()
            .and_then(|content| content.answers.get(form.answer))
        else {
            return Err(PageError::bad_request("no-such-answer"));
        };
        let (label, points) = (answer.label.clone(), answer.points);

        let previous = question.responses.insert(
            user.login.clone(),
            Answer {
                name: user.name.clone(),
                index: form.answer,
                label: label.clone(),
                points,
                answered_at: Utc::now(),
            },
        );

        tracing::info!(
            question = %id,
            login = %user.login,
            answer = %label,
            changed = previous.is_some(),
            total = question.responses.len(),
            "answer recorded"
        );
        // The presentation view counts the answers, the other students in the
        // room see the count as well.
        question.notify();
        store.notify_room_of(&id);
    }
    state.metrics.answer_recorded();

    // Redirect so that reloading the page does not answer again.
    Ok(Redirect::to(&views::link(&format!("/r/{token}"))).into_response())
}

/// `GET /r/{token}/events` - live state of the room: how many have answered,
/// and which question is being asked.
pub async fn events(
    State(state): State<AppState>,
    User(user): User,
    Path(token): Path<String>,
) -> Result<Response, PageError> {
    if state.store.read().await.room(&token).is_none() {
        return Err(not_found());
    }
    let source = live::Source::Room {
        token,
        login: user.login,
    };
    Ok(live::stream(state, source).into_response())
}

fn code_page(
    user: crate::auth::Identity,
    lang: Lang,
    error: String,
) -> Result<String, askama::Error> {
    IndexPage {
        prefix: views::prefix(),
        lang,
        user,
        error,
    }
    .render()
}

fn not_found() -> PageError {
    PageError::not_found("page-gone")
}
