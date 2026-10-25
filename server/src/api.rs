//! The HTTP API the teacher CLI talks to.
//!
//! Every endpoint except [`oidc_info`] requires an OIDC ID token in the
//! `Authorization: Bearer` header; asking questions additionally requires the
//! login to be on the teacher list or the token to carry a teacher role.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use quiz_common::api::{
    CreateQuestion, OidcInfo, QuestionInfo, Results, RoomClosed, RoomStatus, WhoAmI,
};

use crate::{
    auth::{ApiUser, Identity, Teacher},
    error::ApiFailure,
    live,
    state::{AppState, Question, Store},
};

/// `GET /api/v1/oidc` - lets the CLI discover how to log the teacher in, so
/// that it only has to be configured with the server URL.
pub async fn oidc_info(State(state): State<AppState>) -> Json<OidcInfo> {
    let cfg = &state.cfg;
    Json(OidcInfo {
        issuer: cfg.oidc_issuer.as_str().trim_end_matches('/').to_owned(),
        client_id: cfg.cli_client_id().to_owned(),
        redirect_uri: cfg
            .oidc_cli_redirect_uri
            .as_ref()
            .map(|url| url.to_string()),
        scopes: cfg.oidc_scopes.clone(),
    })
}

/// `GET /api/v1/whoami` - who the CLI is logged in as.
pub async fn whoami(State(state): State<AppState>, ApiUser(user): ApiUser) -> Json<WhoAmI> {
    let is_teacher = state.cfg.is_teacher(&user);
    tracing::info!(login = %user.login, roles = ?user.roles, is_teacher, "whoami");
    Json(WhoAmI {
        login: user.login,
        name: user.name,
        is_teacher,
    })
}

/// `POST /api/v1/room/close` - drops the teacher's room and its questions.
///
/// `quiz logout` calls this before it forgets the token: the lecture is over,
/// so the URL and the code students have been using stop working instead of
/// waiting for the next teacher to inherit them.
pub async fn close_room(State(state): State<AppState>, ApiUser(user): ApiUser) -> Json<RoomClosed> {
    let (closed, questions) = state.close_room(&user.login).await;
    if closed {
        tracing::info!(teacher = %user.login, questions, "room closed");
    }
    Json(RoomClosed { closed, questions })
}

/// `GET /api/v1/room` - the teacher's room and the questions kept in it, for
/// `quiz status`.
pub async fn room_status(
    State(state): State<AppState>,
    ApiUser(user): ApiUser,
) -> Json<RoomStatus> {
    let room = state.room_status(&user.login).await;
    tracing::info!(
        teacher = %user.login,
        questions = room.as_ref().map_or(0, |room| room.questions.len()),
        "room status"
    );
    Json(RoomStatus { room })
}

/// `POST /api/v1/questions` - publishes a question read from a Markdown file.
pub async fn create_question(
    State(state): State<AppState>,
    Teacher(teacher): Teacher,
    Json(request): Json<CreateQuestion>,
) -> Result<(StatusCode, Json<QuestionInfo>), ApiFailure> {
    let mut content = quiz_common::parse_question(&request.markdown).map_err(|error| {
        tracing::info!(
            teacher = %teacher.login,
            source = %request.source_name,
            %error,
            "rejected an unparsable question"
        );
        ApiFailure::bad_request(error.to_string())
    })?;

    if request.shuffle {
        quiz_common::question::shuffle_answers(&mut content);
    }

    let id = state
        .add_question(
            source_name(&request.source_name),
            content,
            request.view,
            &teacher.login,
            request.close_after,
        )
        .await?;

    let mut store = state.store.write().await;
    let vanished = || ApiFailure::internal("the question vanished right after it was created");
    if request.start_countdown
        && let Some(at) = store
            .get_mut(&id)
            .ok_or_else(vanished)?
            .start_countdown(Utc::now())
    {
        state.close_at(id.clone(), at);
    }
    let question = store.get(&id).ok_or_else(vanished)?;

    tracing::info!(
        question = %question.id,
        code = %question.code,
        teacher = %teacher.login,
        source = %question.source_name,
        answers = question.content.answers.len(),
        show_question = question.view.show_question,
        show_answers = question.view.show_answers,
        shuffled = request.shuffle,
        close_after = request.close_after,
        answer_url = %question.answer_url,
        display_url = %question.display_url,
        "question created"
    );

    Ok((StatusCode::CREATED, Json(store.question_info(question))))
}

/// `GET /api/v1/questions/{id}` - state of a question, including how many
/// students have answered so far.
pub async fn question(
    State(state): State<AppState>,
    Teacher(teacher): Teacher,
    Path(id): Path<String>,
) -> Result<Json<QuestionInfo>, ApiFailure> {
    let store = state.store.read().await;
    Ok(Json(store.question_info(owned(&store, &id, &teacher)?)))
}

/// `GET /api/v1/questions/{id}/events` - the same as [`question`], sent
/// again over server-sent events whenever it changes, so that the CLI can
/// show how many students have answered without asking over and over.
pub async fn question_events(
    State(state): State<AppState>,
    Teacher(teacher): Teacher,
    Path(id): Path<String>,
) -> Result<Response, ApiFailure> {
    owned(&*state.store.read().await, &id, &teacher)?;
    Ok(live::question_stream(state, id).into_response())
}

/// `POST /api/v1/questions/{id}/close` - stops accepting answers, which also
/// switches every open browser over to the bar graph.
pub async fn close_question(
    State(state): State<AppState>,
    Teacher(teacher): Teacher,
    Path(id): Path<String>,
) -> Result<Json<QuestionInfo>, ApiFailure> {
    let mut store = state.store.write().await;

    // Check ownership before taking the mutable borrow.
    owned(&store, &id, &teacher)?;
    let question = store
        .get_mut(&id)
        .ok_or_else(|| ApiFailure::not_found("no such question"))?;

    if question.close() {
        tracing::info!(
            question = %id,
            teacher = %teacher.login,
            answers = question.responses.len(),
            counts = ?question.counts(),
            "question closed"
        );
    }

    // The students in the room switch over to the results.
    store.notify_room_of(&id);

    Ok(Json(store.question_info(owned(&store, &id, &teacher)?)))
}

/// `POST /api/v1/questions/{id}/reveal` - marks the correct answers in the
/// results, on the projector and on the students' screens.
///
/// Only a closed question can be revealed, so that nobody learns the correct
/// answer while they can still change theirs.
pub async fn reveal_question(
    State(state): State<AppState>,
    Teacher(teacher): Teacher,
    Path(id): Path<String>,
) -> Result<Json<QuestionInfo>, ApiFailure> {
    let mut store = state.store.write().await;

    owned(&store, &id, &teacher)?;
    let question = store
        .get_mut(&id)
        .ok_or_else(|| ApiFailure::not_found("no such question"))?;

    if question.is_open() {
        return Err(ApiFailure::conflict(
            "the question is still open; close it before revealing the answers",
        ));
    }
    if question.reveal() {
        tracing::info!(question = %id, teacher = %teacher.login, "answers revealed");
    }
    store.notify_room_of(&id);

    Ok(Json(store.question_info(owned(&store, &id, &teacher)?)))
}

/// `GET /api/v1/questions/{id}/results` - the collected answers, which the CLI
/// writes to a CSV file.
pub async fn results(
    State(state): State<AppState>,
    Teacher(teacher): Teacher,
    Path(id): Path<String>,
) -> Result<Json<Results>, ApiFailure> {
    let store = state.store.read().await;
    let question = owned(&store, &id, &teacher)?;

    tracing::info!(
        question = %id,
        teacher = %teacher.login,
        rows = question.responses.len(),
        "results exported"
    );

    Ok(Json(question.results()))
}

/// Looks up a question and checks that it belongs to this teacher.
fn owned<'a>(store: &'a Store, id: &str, teacher: &Identity) -> Result<&'a Question, ApiFailure> {
    let question = store
        .get(id)
        .ok_or_else(|| ApiFailure::not_found("no such question"))?;

    if question.teacher != teacher.login {
        tracing::warn!(
            question = %id,
            login = %teacher.login,
            owner = %question.teacher,
            "refused access to somebody else's question"
        );
        return Err(ApiFailure::forbidden(
            "this question was asked by another teacher",
        ));
    }
    Ok(question)
}

/// Keeps only the file name of what the CLI sent, and keeps it short: it ends
/// up in the log.
fn source_name(name: &str) -> String {
    let name = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .trim()
        .trim_matches('.');
    let name = if name.is_empty() { "question.md" } else { name };
    name.chars().take(100).collect()
}

#[cfg(test)]
mod tests {
    use super::source_name;

    #[test]
    fn source_name_keeps_only_the_file_name() {
        assert_eq!(source_name("labs/01regex/questions.md"), "questions.md");
        assert_eq!(source_name("../../etc/passwd"), "passwd");
        assert_eq!(source_name("  "), "question.md");
        assert_eq!(source_name("question.md"), "question.md");
    }
}
