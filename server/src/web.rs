//! The pages students use: the main page, and the page with the question.

use axum::{
    extract::{Form, Path, State},
    response::{Html, IntoResponse, Redirect, Response},
};
use chrono::Utc;
use serde::Deserialize;

use crate::{
    auth::User,
    error::PageError,
    live,
    state::{AppState, Response as Answer},
    views::{self, AnswerPage, IndexPage, bars},
};
use askama::Template;

/// `GET /` - asks for the question code (requirement 10).
pub async fn index(User(user): User) -> Result<Response, PageError> {
    Ok(Html(code_page(user, String::new())?).into_response())
}

#[derive(Debug, Deserialize)]
pub struct JoinForm {
    code: String,
}

/// `POST /join` - sends the student to the URL the QR code points to.
pub async fn join(
    State(state): State<AppState>,
    User(user): User,
    Form(form): Form<JoinForm>,
) -> Result<Response, PageError> {
    // Be forgiving about spaces and dashes in a code read off a projector.
    let code: String = form.code.chars().filter(char::is_ascii_digit).collect();

    let Some(id) = state
        .store
        .read()
        .await
        .by_code(&code)
        .map(|question| question.id.clone())
    else {
        tracing::info!(login = %user.login, %code, "unknown question code");
        return Ok(Html(code_page(
            user,
            format!("Kód {code} nepatří k žádné otázce. Zkuste to prosím znovu."),
        )?)
        .into_response());
    };

    tracing::info!(login = %user.login, %code, question = %id, "student entered a code");
    Ok(Redirect::to(&views::link(&format!("/q/{id}"))).into_response())
}

/// `GET /q/{id}` - the question, and the buttons to answer it.
pub async fn question(
    State(state): State<AppState>,
    User(user): User,
    Path(id): Path<String>,
) -> Result<Response, PageError> {
    let store = state.store.read().await;
    let question = store.get(&id).ok_or_else(not_found)?;

    let chosen = question.responses.get(&user.login).map(|r| r.index);
    let page = AnswerPage {
        prefix: views::prefix(),
        user,
        id: question.id.clone(),
        title: question.title().to_owned(),
        prompt_html: question.content.prompt_html.clone(),
        open: question.is_open(),
        answer_count: question.responses.len(),
        chosen_label: chosen
            .and_then(|index| question.content.answers.get(index))
            .map(|answer| answer.label.clone())
            .unwrap_or_default(),
        bars: bars(question, chosen),
    };

    Ok(Html(page.render()?).into_response())
}

#[derive(Debug, Deserialize)]
pub struct AnswerForm {
    answer: usize,
}

/// `POST /q/{id}/answer` - records one answer.
pub async fn answer(
    State(state): State<AppState>,
    User(user): User,
    Path(id): Path<String>,
    Form(form): Form<AnswerForm>,
) -> Result<Response, PageError> {
    {
        let mut store = state.store.write().await;
        let question = store.get_mut(&id).ok_or_else(not_found)?;

        if !question.is_open() {
            tracing::info!(question = %id, login = %user.login, "answer to a closed question");
            return Err(PageError::new(
                axum::http::StatusCode::CONFLICT,
                "Otázka je uzavřena",
                "Otázka už nepřijímá odpovědi.",
            ));
        }

        let Some(answer) = question.content.answers.get(form.answer) else {
            return Err(PageError::bad_request("Taková odpověď neexistuje."));
        };
        let label = answer.label.clone();

        let previous = question.responses.insert(
            user.login.clone(),
            Answer {
                name: user.name.clone(),
                index: form.answer,
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
        question.notify();
    }

    // Redirect so that reloading the page does not answer again.
    Ok(Redirect::to(&views::link(&format!("/q/{id}"))).into_response())
}

/// `GET /q/{id}/events` - live number of answers for the student page.
pub async fn events(
    State(state): State<AppState>,
    User(_user): User,
    Path(id): Path<String>,
) -> Result<Response, PageError> {
    if state.store.read().await.get(&id).is_none() {
        return Err(not_found());
    }
    Ok(live::stream(state, id).into_response())
}

fn code_page(user: crate::auth::Identity, error: String) -> Result<String, askama::Error> {
    IndexPage {
        prefix: views::prefix(),
        user,
        error,
    }
    .render()
}

fn not_found() -> PageError {
    PageError::not_found("Otázka neexistuje, nebo už byla ze serveru odstraněna.")
}
