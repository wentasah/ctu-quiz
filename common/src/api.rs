//! JSON payloads of the HTTP API used by the teacher CLI.
//!
//! All API endpoints live under [`API_PREFIX`] and, except for
//! [`OidcInfo`](crate::api::OidcInfo), require an OIDC ID token in the
//! `Authorization: Bearer ...` header.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Common prefix of all API endpoints.
pub const API_PREFIX: &str = "/api/v1";

/// `GET /api/v1/oidc` - what the CLI needs in order to log the teacher in.
///
/// Publishing this lets the CLI be configured with the server URL alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcInfo {
    /// Issuer URL, used for OIDC discovery.
    pub issuer: String,
    /// Client ID the CLI should use.
    pub client_id: String,
    /// Redirect URI registered for the CLI, if the provider requires a fixed
    /// one. `None` means any `http://127.0.0.1:<port>/callback` will do.
    pub redirect_uri: Option<String>,
    /// Scopes to request.
    pub scopes: Vec<String>,
}

/// `POST /api/v1/questions` - publish a question read from a Markdown file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateQuestion {
    /// File name the question came from, for the log and the page title.
    pub source_name: String,
    /// The Markdown source; the server parses it with
    /// [`quiz_common::parse_question`](crate::parse_question).
    pub markdown: String,
}

/// One possible answer as shown to students.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnswerInfo {
    pub label: String,
    /// Plain-text form of the answer.
    pub text: String,
}

/// State of a question. Returned when a question is created and by
/// `GET /api/v1/questions/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionInfo {
    pub id: String,
    /// Short numeric code students type on the main page.
    pub code: String,
    pub title: Option<String>,
    /// Name of the Markdown file the question was read from.
    pub source_name: String,
    /// URL students are sent to, also encoded in the QR code.
    pub answer_url: String,
    /// Capability URL of the presentation view; keep it secret.
    pub display_url: String,
    pub answers: Vec<AnswerInfo>,
    /// Whether the question still accepts answers.
    pub open: bool,
    /// Number of students who have answered.
    pub answer_count: usize,
    /// Number of answers per possible answer, in the order of `answers`.
    pub counts: Vec<usize>,
    pub teacher: String,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

/// Live state pushed to browsers over server-sent events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveState {
    pub open: bool,
    pub answer_count: usize,
    /// Per-answer counts. Only revealed once the question is closed, so that
    /// students cannot watch the distribution while they are answering.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counts: Option<Vec<usize>>,
    /// Answer labels, to keep the browser from having to know them up front.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
}

/// One row of the collected answers, and one row of the exported CSV.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultRow {
    /// Login of the student, taken from the OIDC token.
    pub login: String,
    /// Full name, if the OIDC provider gives one.
    pub name: String,
    /// `A`, `B`, ...
    pub answer: String,
    /// Plain text of the chosen answer.
    pub answer_text: String,
    pub answered_at: DateTime<Utc>,
}

/// `GET /api/v1/questions/{id}/results`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Results {
    pub question: QuestionInfo,
    /// Rows sorted by login.
    pub rows: Vec<ResultRow>,
}

/// `GET /api/v1/whoami` - lets the CLI show who it is logged in as.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhoAmI {
    pub login: String,
    pub name: String,
    pub is_teacher: bool,
}

/// Body of every error response of the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
}
