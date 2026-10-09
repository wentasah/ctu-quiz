//! Error types of the page and API handlers.

use askama::Template;
use axum::{
    Json,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use quiz_common::api::ApiError;

use crate::views::{MessagePage, prefix};

/// Error of a handler that serves HTML: shown to the user as a page.
#[derive(Debug)]
pub struct PageError {
    status: StatusCode,
    title: &'static str,
    message: String,
}

impl PageError {
    pub fn new(status: StatusCode, title: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            title,
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "Nenalezeno", message)
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "Chybný požadavek", message)
    }

    /// Logs the cause and shows a generic message, so that internal details
    /// stay in the log file.
    pub fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(%error, "request failed");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Chyba serveru",
            "Požadavek se nepodařilo zpracovat. Zkuste to prosím znovu.",
        )
    }
}

impl IntoResponse for PageError {
    fn into_response(self) -> Response {
        let page = MessagePage {
            prefix: prefix(),
            title: self.title.to_owned(),
            message: self.message.clone(),
        };
        match page.render() {
            Ok(html) => (self.status, Html(html)).into_response(),
            Err(error) => {
                tracing::error!(%error, "rendering the error page failed");
                (self.status, format!("{}: {}", self.title, self.message)).into_response()
            }
        }
    }
}

impl From<anyhow::Error> for PageError {
    fn from(error: anyhow::Error) -> Self {
        Self::internal(format!("{error:#}"))
    }
}

impl From<askama::Error> for PageError {
    fn from(error: askama::Error) -> Self {
        Self::internal(error)
    }
}

/// Error of an API handler: reported as JSON, as the CLI expects.
#[derive(Debug)]
pub struct ApiFailure {
    status: StatusCode,
    message: String,
}

impl ApiFailure {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    pub fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(%error, "API request failed");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error, see the server log",
        )
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiError {
                error: self.message,
            }),
        )
            .into_response()
    }
}

impl From<anyhow::Error> for ApiFailure {
    fn from(error: anyhow::Error) -> Self {
        Self::internal(format!("{error:#}"))
    }
}
