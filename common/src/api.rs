//! JSON payloads of the HTTP API used by the teacher CLI.
//!
//! All API endpoints live under [`API_PREFIX`] and, except for
//! [`OidcInfo`](crate::api::OidcInfo), require an OIDC ID token in the
//! `Authorization: Bearer ...` header.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::Images;

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

/// What of a question students see on their own device.
///
/// The question and the answers are on the teacher's screen in any case;
/// keeping them off the phones makes it harder to paste the question into a
/// chatbot, and the students still have their buttons to answer with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StudentView {
    /// Whether the question text is shown to students.
    pub show_question: bool,
    /// Whether the answers are shown with their text. Without it a student
    /// sees the labels `A`, `B`, `C`, ... alone.
    pub show_answers: bool,
}

impl Default for StudentView {
    /// Everything, which is what a question without an opinion gets.
    fn default() -> Self {
        Self {
            show_question: true,
            show_answers: true,
        }
    }
}

/// `POST /api/v1/questions` - publish a question read from a Markdown file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateQuestion {
    /// File name the question came from, for the log.
    pub source_name: String,
    /// The Markdown source; the server parses it with
    /// [`quiz_common::parse_with_images`](crate::parse_with_images).
    pub markdown: String,
    /// The local images the Markdown shows, keyed by their path as written
    /// in it; see [`crate::question::local_images`].
    #[serde(default, skip_serializing_if = "Images::is_empty")]
    pub images: Images,
    /// What students see on their own device; everything by default, which
    /// is also what an older CLI that does not send it gets.
    #[serde(default)]
    pub view: StudentView,
    /// Whether the server shuffles the answers before showing them. The
    /// labels are handed out after the shuffle, so `A` is the first answer
    /// on the screen either way.
    #[serde(default)]
    pub shuffle: bool,
    /// Seconds after which the server closes the question by itself, counted
    /// from when the presentation view first shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_after: Option<u64>,
    /// Whether the `close_after` countdown starts right away instead. The
    /// CLI asks for that with `--no-browser`, when it cannot tell whether
    /// anybody is going to open the presentation view.
    #[serde(default)]
    pub start_countdown: bool,
}

/// One possible answer as shown to students.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnswerInfo {
    pub label: String,
    /// Plain-text form of the answer.
    pub text: String,
    /// Points for this answer, if the question is scored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points: Option<f64>,
}

/// State of a question. Returned when a question is created and by
/// `GET /api/v1/questions/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionInfo {
    pub id: String,
    /// What students see of this question on their own device.
    #[serde(default)]
    pub view: StudentView,
    /// Short numeric code students type on the main page.
    ///
    /// It belongs to the teacher's room, not to the single question, so it
    /// stays the same for every question the teacher asks.
    pub code: String,
    pub title: Option<String>,
    /// Name of the Markdown file the question was read from.
    pub source_name: String,
    /// URL students are sent to, also encoded in the QR code. It is the URL
    /// of the teacher's room, so students who are already on it see this
    /// question without scanning anything again.
    pub answer_url: String,
    /// Capability URL of the presentation view; keep it secret. It is the
    /// same for every question of the room and shows the current one.
    pub display_url: String,
    /// Whether the presentation view was already open in a browser when the
    /// question was asked, in which case it shows the question by itself.
    #[serde(default)]
    pub presented: bool,
    pub answers: Vec<AnswerInfo>,
    /// Whether the question still accepts answers.
    pub open: bool,
    /// Number of students who have answered.
    pub answer_count: usize,
    /// Number of students in the room: those who have its page open and
    /// those who have answered, so never fewer than `answer_count`.
    #[serde(default)]
    pub people: usize,
    /// Number of answers per possible answer, in the order of `answers`.
    pub counts: Vec<usize>,
    pub teacher: String,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    /// Whether the correct answers are shown with the results.
    #[serde(default)]
    pub revealed: bool,
    /// Whether this is no question at all but points the teacher gave by
    /// hand with `quiz award`; see [`AwardPoints`].
    #[serde(default)]
    pub manual: bool,
}

/// Live state pushed to browsers over server-sent events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveState {
    /// Id of the question being asked, `None` while the room is waiting for
    /// the next one. A change means the page has to be re-rendered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    pub open: bool,
    /// Whether the results show which answers are correct. A change means
    /// the page has to be re-rendered.
    #[serde(default)]
    pub revealed: bool,
    pub answer_count: usize,
    /// Per-answer counts. Only revealed once the question is closed, so that
    /// students cannot watch the distribution while they are answering.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counts: Option<Vec<usize>>,
    /// Answer labels, to keep the browser from having to know them up front.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
}

impl LiveState {
    /// The state of a room in which no question is being asked.
    pub fn waiting() -> Self {
        Self {
            question: None,
            open: false,
            revealed: false,
            answer_count: 0,
            counts: None,
            labels: None,
        }
    }
}

/// `POST /api/v1/room/close` - what became of the teacher's room.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomClosed {
    /// Whether there was a room to close.
    pub closed: bool,
    /// How many questions were dropped together with it.
    pub questions: usize,
}

/// `POST /api/v1/room/award` - award a student points by hand, for
/// something done outside the quiz.
///
/// The points go into a manual entry of the room named `label`, which the
/// room CSV shows as a column of its own. A real question is never changed:
/// when `label` is the name of one, or a manual entry of that name has points
/// of the student already, the points go to `label#2`, `label#3`, ...
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwardPoints {
    pub login: String,
    pub points: f64,
    /// [`DEFAULT_POINTS_LABEL`] when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Name of the manual entry of [`AwardPoints`] without a label.
pub const DEFAULT_POINTS_LABEL: &str = "manual";

/// What became of [`AwardPoints`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PointsAwarded {
    /// The manual entry the points went to.
    pub question: QuestionInfo,
    /// Name of the student, as known from their answers; the login if they
    /// have answered nothing.
    pub name: String,
}

/// `GET /api/v1/room` - the teacher's room and what is kept of it, as `quiz
/// status` shows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomStatus {
    /// `None` when the teacher has no room open.
    pub room: Option<RoomInfo>,
}

/// A room, see [`RoomStatus`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomInfo {
    /// Short numeric code students type on the main page.
    pub code: String,
    /// URL students are sent to, also encoded in the QR code.
    pub answer_url: String,
    /// Capability URL of the presentation view; keep it secret.
    pub display_url: String,
    /// Whether a browser shows the presentation view right now.
    pub presented: bool,
    /// Number of students who have the room open in a browser right now.
    pub students: usize,
    /// Id of the question the students see, `None` when there is none.
    pub current: Option<String>,
    pub created_at: DateTime<Utc>,
    /// When the teacher last asked a question here.
    pub last_used_at: DateTime<Utc>,
    /// When the server drops the room unless another question is asked
    /// before; it checks every few minutes, so it may happen a bit later.
    pub expires_at: DateTime<Utc>,
    /// Questions still kept in memory, in the order they were asked.
    pub questions: Vec<RoomQuestion>,
    /// Every student who is in the room now or answered a question kept in
    /// it, sorted by login; empty from older servers.
    #[serde(default)]
    pub roster: Vec<RoomStudent>,
}

/// A student in a room, see [`RoomInfo::roster`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomStudent {
    pub login: String,
    /// Full name, as given with an answer; empty for a student who has not
    /// answered any of the kept questions.
    pub name: String,
    /// Whether the student has the room open in a browser right now.
    pub present: bool,
    /// How many of the questions kept in the room the student answered;
    /// manual entries do not count.
    pub answered: usize,
    /// Points for those answers and the manual ones, `None` when none of
    /// them is scored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points: Option<f64>,
}

/// A question kept in a room, see [`RoomInfo`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomQuestion {
    /// Order of the question in its room, starting from 1.
    pub number: usize,
    /// When the server drops the question, like [`RoomInfo::expires_at`].
    pub expires_at: DateTime<Utc>,
    #[serde(flatten)]
    pub info: QuestionInfo,
}

/// One row of the collected answers, and most of one row of the exported
/// CSV.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultRow {
    /// Login of the student, taken from the OIDC token.
    pub login: String,
    /// Full name, if the OIDC provider gives one.
    pub name: String,
    /// Points for the chosen answer; left out, also from the CSV, when the
    /// question is not scored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points: Option<f64>,
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
