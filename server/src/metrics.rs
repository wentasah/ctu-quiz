//! Prometheus metrics, served at `/metrics` in the text exposition format.
//!
//! The counters only ever grow while the process runs; the gauges are
//! computed from the state at the time of the scrape. The format is simple
//! enough to be written by hand, which spares a dependency.

use std::{
    collections::{BTreeSet, HashMap},
    fmt::Write,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{extract::State, http::header, response::IntoResponse};

use crate::state::AppState;

/// How long after its last API request a teacher still counts as connected.
/// `quiz ask` asks every second while it waits for answers.
pub const TEACHER_IDLE: Duration = Duration::from_secs(30);

/// What the server counts as it goes.
#[derive(Debug, Default)]
pub struct Metrics {
    questions: AtomicU64,
    answers: AtomicU64,
    /// Login of a teacher to the time of their last API request.
    teachers_seen: Mutex<HashMap<String, Instant>>,
}

impl Metrics {
    pub fn question_asked(&self) {
        self.questions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn answer_recorded(&self) {
        self.answers.fetch_add(1, Ordering::Relaxed);
    }

    /// Notes that `teacher` has just used the API.
    pub fn teacher_seen(&self, teacher: &str) {
        let now = Instant::now();
        let mut seen = self.lock_seen();
        // Forgetting the idle ones here keeps the map as small as the number
        // of teachers who are active right now.
        seen.retain(|_, last| now.duration_since(*last) < TEACHER_IDLE);
        seen.insert(teacher.to_owned(), now);
    }

    /// Teachers who have used the API within [`TEACHER_IDLE`].
    fn active_teachers(&self) -> BTreeSet<String> {
        let now = Instant::now();
        self.lock_seen()
            .iter()
            .filter(|(_, last)| now.duration_since(**last) < TEACHER_IDLE)
            .map(|(login, _)| login.clone())
            .collect()
    }

    fn lock_seen(&self) -> std::sync::MutexGuard<'_, HashMap<String, Instant>> {
        self.teachers_seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// `GET /metrics`
pub async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let (rooms, mut teachers, students) = {
        let store = state.store.read().await;
        (
            store.room_count(),
            store.presenting_teachers(),
            store.connected_students(),
        )
    };
    teachers.extend(state.metrics.active_teachers());

    let mut body = String::new();
    let mut metric = |name: &str, kind: &str, help: &str, value: u64| {
        let _ = writeln!(body, "# HELP {name} {help}");
        let _ = writeln!(body, "# TYPE {name} {kind}");
        let _ = writeln!(body, "{name} {value}");
    };
    metric(
        "quiz_questions_asked_total",
        "counter",
        "Questions asked since the server started.",
        state.metrics.questions.load(Ordering::Relaxed),
    );
    metric(
        "quiz_answers_total",
        "counter",
        "Answers recorded since the server started, changed answers included.",
        state.metrics.answers.load(Ordering::Relaxed),
    );
    metric(
        "quiz_rooms_open",
        "gauge",
        "Rooms that are open.",
        rooms as u64,
    );
    metric(
        "quiz_teachers_connected",
        "gauge",
        "Teachers with the presentation view open or who used the API in the last 30 seconds.",
        teachers.len() as u64,
    );
    metric(
        "quiz_students_connected",
        "gauge",
        "Students with a room open in a browser.",
        students as u64,
    );

    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}
