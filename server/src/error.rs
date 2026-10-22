//! Error types of the page and API handlers.

use askama::Template;
use axum::{
    Json,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use quiz_common::api::ApiError;

use crate::{
    i18n::Lang,
    views::{MessagePage, prefix},
};

/// Error of a handler that serves HTML: shown to the user as a page.
///
/// It holds the ids of its messages rather than their texts: the
/// [`negotiate`](crate::i18n::negotiate) middleware renders the page in the
/// language of the request.
#[derive(Debug, Clone)]
pub struct PageError {
    status: StatusCode,
    title: &'static str,
    message: &'static str,
    args: Vec<(&'static str, String)>,
}

impl PageError {
    pub fn new(status: StatusCode, title: &'static str, message: &'static str) -> Self {
        Self {
            status,
            title,
            message,
            args: Vec::new(),
        }
    }

    /// Gives the message an argument.
    pub fn arg(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.args.push((name, value.into()));
        self
    }

    pub fn not_found(message: &'static str) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not-found", message)
    }

    pub fn bad_request(message: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad-request", message)
    }

    /// Logs the cause and shows a generic message, so that internal details
    /// stay in the log file.
    pub fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(%error, "request failed");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "server-error",
            "server-error-hint",
        )
    }

    /// The error page in `lang`.
    pub fn render(&self, lang: Lang) -> Response {
        let message = self
            .args
            .iter()
            .fold(lang.msg(self.message), |msg, (name, value)| {
                msg.arg(name, value.as_str())
            });
        let page = MessagePage {
            prefix: prefix(),
            lang,
            title: lang.t(self.title),
            message: message.to_string(),
        };
        match page.render() {
            Ok(html) => (self.status, Html(html)).into_response(),
            Err(error) => {
                tracing::error!(%error, "rendering the error page failed");
                (self.status, format!("{}: {}", page.title, page.message)).into_response()
            }
        }
    }
}

impl IntoResponse for PageError {
    /// Renders the page in the default language, and leaves the error in the
    /// response for the middleware to render it again in the right one.
    fn into_response(self) -> Response {
        let mut response = self.render(Lang::default());
        response.extensions_mut().insert(self);
        response
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
