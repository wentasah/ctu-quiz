//! In-memory state of the server.
//!
//! The assignment forbids on-disc state, so questions, collected answers and
//! browser sessions live in this process only and are lost on restart. The log
//! file is written but never read back. Only when asked to with
//! `--state-file` are the rooms, questions and answers carried over a restart,
//! see [`crate::persist`].
//!
//! Questions are grouped into [`Room`]s, one per teacher. A room is what the
//! QR code and the numeric code point at, and it outlives the single question:
//! students scan once at the beginning of a lecture and every further question
//! of that teacher appears on the page they already have open.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::Deref,
    sync::{Arc, Mutex, PoisonError},
};

use anyhow::Result;
use axum::extract::FromRef;
use axum_extra::extract::cookie::Key;
use chrono::{DateTime, TimeDelta, Utc};
use openidconnect::{Nonce, PkceCodeVerifier};
use quiz_common::api::{
    AnswerInfo, LiveState, QuestionInfo, ResultRow, Results, RoomInfo, RoomQuestion, StudentView,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

use crate::{
    auth::{Identity, Oidc},
    config::Config,
    metrics::Metrics,
};

/// How long a login that was started may still be finished.
pub const LOGIN_TTL_MINUTES: i64 = 15;

/// How many logins may be in progress at once.
///
/// `GET /login` needs no authentication and remembers something for every
/// visit, so without a limit anybody could fill the server's memory by asking
/// for logins they never finish.
pub const MAX_PENDING_LOGINS: usize = 4096;

/// How many questions a room keeps. Asking one more drops the oldest, so that
/// a teacher asking in a loop cannot fill the server's memory either.
pub const MAX_ROOM_QUESTIONS: usize = 100;

/// A login started in a browser and not yet finished.
pub struct PendingLogin {
    pub pkce_verifier: PkceCodeVerifier,
    pub nonce: Nonce,
    /// Path to send the browser to once the login succeeds.
    pub next: String,
    pub created_at: DateTime<Utc>,
}

/// Remembers a login that has just been started, unless too many are already
/// in progress. Returns whether it was remembered.
///
/// A full table is first rid of the logins that have expired anyway; only if
/// that does not help is the new one refused.
fn insert_pending_login(
    logins: &mut HashMap<String, PendingLogin>,
    csrf: String,
    pending: PendingLogin,
) -> bool {
    if logins.len() >= MAX_PENDING_LOGINS {
        let cutoff = pending.created_at - login_ttl();
        logins.retain(|_, login| login.created_at > cutoff);
    }
    if logins.len() >= MAX_PENDING_LOGINS {
        return false;
    }
    logins.insert(csrf, pending);
    true
}

fn login_ttl() -> TimeDelta {
    TimeDelta::try_minutes(LOGIN_TTL_MINUTES).unwrap_or(TimeDelta::zero())
}

/// A logged-in browser.
#[derive(Debug, Clone)]
pub struct Session {
    pub identity: Identity,
    pub expires_at: DateTime<Utc>,
}

/// One student's answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub name: String,
    pub index: usize,
    pub answered_at: DateTime<Utc>,
}

/// A room: where a teacher's students are, for as long as the lecture lasts.
///
/// The room owns everything students need to reach: its token is in the URL
/// of the QR code, and its code is the one shown next to it. Which question
/// they see is [`Room::current`], which the teacher changes by asking another
/// one.
pub struct Room {
    /// Unguessable part of the URL students stay on.
    pub token: String,
    /// Short code students type on the main page.
    pub code: String,
    /// Login of the teacher whose room this is.
    pub teacher: String,
    /// URL of the room, as printed by the CLI and encoded in the QR code.
    pub url: String,
    /// Secret part of the presentation URL.
    pub display_token: String,
    /// URL of the presentation view, which shows whatever question is being
    /// asked here, so one browser window serves the whole lecture.
    pub display_url: String,
    /// Held by every open event stream of the presentation view, so that
    /// the count of its holders tells whether one is open.
    pub presenters: Arc<()>,
    /// Students who have the room open in a browser right now.
    pub students: Presence,
    /// The question being asked right now, if any.
    pub current: Option<String>,
    /// How many questions have been asked here; numbers the next one.
    pub asked: usize,
    pub created_at: DateTime<Utc>,
    /// When the teacher last asked a question here.
    pub last_used_at: DateTime<Utc>,
    /// Notifies the pages of the students in this room.
    pub updates: broadcast::Sender<()>,
}

impl Room {
    /// Wakes up the event streams of the students in this room.
    pub fn notify(&self) {
        // An error only means nobody is watching.
        let _ = self.updates.send(());
    }

    /// Whether a browser shows the presentation view of this room.
    pub fn is_presented(&self) -> bool {
        Arc::strong_count(&self.presenters) > 1
    }

    /// How many people are in the room while `question` is asked: those
    /// who have the room open, and those who answered and have left since.
    pub fn people(&self, question: &Question) -> usize {
        let mut logins = self.students.logins();
        logins.extend(question.responses.keys().cloned());
        logins.len()
    }
}

/// Logins of the students whose event streams of a room are open, each with
/// the number of its streams: a student with two tabs is still one student.
#[derive(Debug, Clone)]
pub struct Presence {
    logins: Arc<Mutex<HashMap<String, usize>>>,
    /// Pinged whenever a student comes or goes, for the teacher's count of
    /// the people in the room.
    changes: broadcast::Sender<()>,
}

impl Default for Presence {
    fn default() -> Self {
        Self {
            logins: Arc::default(),
            changes: broadcast::channel(16).0,
        }
    }
}

impl Presence {
    /// Counts `login` as present until the returned guard is dropped.
    pub fn enter(&self, login: &str) -> Present {
        let mut logins = self.lock();
        let streams = logins.entry(login.to_owned()).or_default();
        *streams += 1;
        if *streams == 1 {
            self.changed();
        }
        Present {
            presence: self.clone(),
            login: login.to_owned(),
        }
    }

    pub fn logins(&self) -> BTreeSet<String> {
        self.lock().keys().cloned().collect()
    }

    /// Wakes up whoever follows who is in the room.
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.changes.subscribe()
    }

    fn changed(&self) {
        // An error only means nobody is watching.
        let _ = self.changes.send(());
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, usize>> {
        // The map stays consistent even if a holder panicked.
        self.logins.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One open event stream of a student, see [`Presence::enter`].
#[derive(Debug)]
pub struct Present {
    presence: Presence,
    login: String,
}

impl Drop for Present {
    fn drop(&mut self) {
        let mut logins = self.presence.lock();
        if let Some(streams) = logins.get_mut(&self.login) {
            *streams -= 1;
            if *streams == 0 {
                logins.remove(&self.login);
                self.presence.changed();
            }
        }
    }
}

/// A question asked by a teacher.
pub struct Question {
    pub id: String,
    /// Token of the room the question was asked in.
    pub room: String,
    /// Short code students type on the main page; the room's.
    pub code: String,
    /// Name of the Markdown file the question came from.
    pub source_name: String,
    /// Order of the question in its room, starting from 1.
    pub number: usize,
    pub content: quiz_common::Question,
    /// What of this question students see on their own device.
    pub view: StudentView,
    pub answer_url: String,
    /// The room's presentation URL.
    pub display_url: String,
    /// Whether the presentation view was already open in a browser when the
    /// question was asked, and so switched over to it by itself.
    pub presented: bool,
    pub teacher: String,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    /// Whether the teacher has revealed which answers are right; only ever
    /// after the question is closed.
    pub revealed: bool,
    /// Answers, keyed by login, so that answering twice replaces the answer.
    pub responses: BTreeMap<String, Response>,
    /// Notifies the server-sent-event streams that something changed.
    pub updates: broadcast::Sender<()>,
}

impl Question {
    pub fn is_open(&self) -> bool {
        self.closed_at.is_none()
    }

    /// How many students picked each of the possible answers.
    pub fn counts(&self) -> Vec<usize> {
        let mut counts = vec![0; self.content.answers.len()];
        for response in self.responses.values() {
            if let Some(count) = counts.get_mut(response.index) {
                *count += 1;
            }
        }
        counts
    }

    pub fn info(&self) -> QuestionInfo {
        QuestionInfo {
            id: self.id.clone(),
            view: self.view,
            code: self.code.clone(),
            title: self.content.title.clone(),
            source_name: self.source_name.clone(),
            answer_url: self.answer_url.clone(),
            display_url: self.display_url.clone(),
            presented: self.presented,
            answers: self
                .content
                .answers
                .iter()
                .map(|a| AnswerInfo {
                    label: a.label.clone(),
                    text: a.plain.clone(),
                    points: a.points,
                })
                .collect(),
            open: self.is_open(),
            answer_count: self.responses.len(),
            people: self.responses.len(),
            counts: self.counts(),
            teacher: self.teacher.clone(),
            created_at: self.created_at,
            closed_at: self.closed_at,
            revealed: self.revealed,
        }
    }

    /// State pushed to browsers. Per-answer counts are only revealed once the
    /// question is closed.
    pub fn live_state(&self) -> LiveState {
        let closed = !self.is_open();
        LiveState {
            question: Some(self.id.clone()),
            open: self.is_open(),
            revealed: self.revealed,
            answer_count: self.responses.len(),
            counts: closed.then(|| self.counts()),
            labels: closed.then(|| {
                self.content
                    .answers
                    .iter()
                    .map(|a| a.label.clone())
                    .collect()
            }),
        }
    }

    pub fn results(&self) -> Results {
        let rows = self
            .responses
            .iter()
            .map(|(login, response)| {
                let answer = self.content.answers.get(response.index);
                ResultRow {
                    login: login.clone(),
                    name: response.name.clone(),
                    answer: answer.map(|a| a.label.clone()).unwrap_or_default(),
                    answer_text: answer.map(|a| a.plain.clone()).unwrap_or_default(),
                    points: answer.and_then(|a| a.points),
                    answered_at: response.answered_at,
                }
            })
            .collect();
        Results {
            question: self.info(),
            rows,
        }
    }

    /// Wakes up the event streams of this question.
    pub fn notify(&self) {
        // An error only means nobody is watching.
        let _ = self.updates.send(());
    }
}

/// All rooms and all questions, with the indexes needed to find them.
#[derive(Default)]
pub struct Store {
    questions: HashMap<String, Question>,
    rooms: HashMap<String, Room>,
    /// Numeric code of a room to its token.
    by_code: HashMap<String, String>,
    /// Login of a teacher to the token of their room.
    by_teacher: HashMap<String, String>,
    /// Display token of a room to its token.
    by_display: HashMap<String, String>,
}

impl Store {
    pub fn get(&self, id: &str) -> Option<&Question> {
        self.questions.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Question> {
        self.questions.get_mut(id)
    }

    /// Looks up a room from the token in the student URL.
    pub fn room(&self, token: &str) -> Option<&Room> {
        self.rooms.get(token)
    }

    pub fn room_mut(&mut self, token: &str) -> Option<&mut Room> {
        self.rooms.get_mut(token)
    }

    /// Looks up the room a student typed the code of.
    pub fn room_by_code(&self, code: &str) -> Option<&Room> {
        self.by_code
            .get(code)
            .and_then(|token| self.rooms.get(token))
    }

    /// The room of a teacher, if they have already asked something.
    pub fn room_of_teacher(&self, teacher: &str) -> Option<&Room> {
        self.by_teacher
            .get(teacher)
            .and_then(|token| self.rooms.get(token))
    }

    /// The question a room is asking right now.
    pub fn current_question(&self, token: &str) -> Option<&Question> {
        let room = self.room(token)?;
        room.current.as_ref().and_then(|id| self.questions.get(id))
    }

    /// What the teacher learns about a question, including how many people
    /// are in its room.
    pub fn question_info(&self, question: &Question) -> QuestionInfo {
        let mut info = question.info();
        if let Some(room) = self.room(&question.room) {
            info.people = room.people(question);
        }
        info
    }

    /// The room of `teacher` with the questions kept in it, for `quiz
    /// status`. Both are dropped `ttl` after they were last used, see
    /// [`Store::prune`].
    pub fn room_status(&self, teacher: &str, ttl: TimeDelta) -> Option<RoomInfo> {
        let room = self.room_of_teacher(teacher)?;
        let mut questions: Vec<RoomQuestion> = self
            .questions
            .values()
            .filter(|question| question.room == room.token)
            .map(|question| RoomQuestion {
                number: question.number,
                expires_at: question.created_at + ttl,
                info: self.question_info(question),
            })
            .collect();
        questions.sort_by_key(|question| question.number);

        Some(RoomInfo {
            code: room.code.clone(),
            answer_url: room.url.clone(),
            display_url: room.display_url.clone(),
            presented: room.is_presented(),
            students: room.students.logins().len(),
            current: room.current.clone(),
            created_at: room.created_at,
            last_used_at: room.last_used_at,
            expires_at: room.last_used_at + ttl,
            questions,
        })
    }

    /// What the students in a room are shown, for the event stream.
    pub fn room_state(&self, token: &str) -> Option<LiveState> {
        self.room(token)?;
        Some(match self.current_question(token) {
            Some(question) => question.live_state(),
            None => LiveState::waiting(),
        })
    }

    /// Tells the students in the room a question belongs to that something
    /// about it changed.
    pub fn notify_room_of(&self, question: &str) {
        if let Some(room) = self.get(question).and_then(|q| self.room(&q.room)) {
            room.notify();
        }
    }

    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }

    /// Teachers whose presentation view is open in a browser.
    pub fn presenting_teachers(&self) -> BTreeSet<String> {
        self.rooms
            .values()
            .filter(|room| room.is_presented())
            .map(|room| room.teacher.clone())
            .collect()
    }

    /// How many students have a room open in a browser; one who has two
    /// rooms open is still one student.
    pub fn connected_students(&self) -> usize {
        let mut logins = BTreeSet::new();
        for room in self.rooms.values() {
            logins.extend(room.students.logins());
        }
        logins.len()
    }

    /// Looks up a room from the secret part of its presentation URL.
    pub fn room_by_display_token(&self, token: &str) -> Option<&Room> {
        self.by_display
            .get(token)
            .and_then(|token| self.rooms.get(token))
    }

    pub fn insert_room(&mut self, room: Room) {
        self.by_code.insert(room.code.clone(), room.token.clone());
        self.by_display
            .insert(room.display_token.clone(), room.token.clone());
        self.by_teacher
            .insert(room.teacher.clone(), room.token.clone());
        self.rooms.insert(room.token.clone(), room);
    }

    pub fn rooms(&self) -> impl Iterator<Item = &Room> {
        self.rooms.values()
    }

    pub fn questions(&self) -> impl Iterator<Item = &Question> {
        self.questions.values()
    }

    pub fn insert_question(&mut self, question: Question) {
        self.questions.insert(question.id.clone(), question);
    }

    /// Drops the oldest questions of the room `token` until at most `max`
    /// are left. Returns how many were dropped.
    fn trim_room(&mut self, token: &str, max: usize) -> usize {
        let mut asked: Vec<(usize, String)> = self
            .questions
            .values()
            .filter(|question| question.room == token)
            .map(|question| (question.number, question.id.clone()))
            .collect();
        if asked.len() <= max {
            return 0;
        }
        asked.sort_unstable();
        let excess = asked.len() - max;
        for (_, id) in &asked[..excess] {
            self.questions.remove(id);
        }
        if let Some(room) = self.rooms.get_mut(token)
            && room
                .current
                .as_ref()
                .is_some_and(|id| !self.questions.contains_key(id))
        {
            room.current = None;
        }
        excess
    }

    /// Removes the room of `teacher` together with its questions, and tells
    /// the students in it to have another look. Returns how many questions
    /// went with it, or `None` when the teacher had no room.
    fn remove_room_of_teacher(&mut self, teacher: &str) -> Option<usize> {
        let token = self.by_teacher.remove(teacher)?;
        let room = self.rooms.remove(&token)?;
        self.by_code.remove(&room.code);
        self.by_display.remove(&room.display_token);

        let asked: Vec<String> = self
            .questions
            .values()
            .filter(|question| question.room == token)
            .map(|question| question.id.clone())
            .collect();
        for id in &asked {
            self.questions.remove(id);
        }

        // The streams find nothing and tell the pages, the presentation view
        // included, to reload, which then says that the room is no longer
        // there.
        room.notify();
        Some(asked.len())
    }

    /// A six-digit code that is not in use by any room.
    fn free_code(&self) -> String {
        loop {
            let number = u128::from_be_bytes(*Uuid::new_v4().as_bytes()) % 1_000_000;
            let code = format!("{number:06}");
            if !self.by_code.contains_key(&code) {
                return code;
            }
        }
    }

    /// Drops questions created before `cutoff`, and the rooms that have not
    /// been used since then. Returns how many questions were dropped.
    pub(crate) fn prune(&mut self, cutoff: DateTime<Utc>) -> usize {
        let expired: Vec<String> = self
            .questions
            .values()
            .filter(|q| q.created_at < cutoff)
            .map(|q| q.id.clone())
            .collect();
        for id in &expired {
            self.questions.remove(id);
        }

        let abandoned: Vec<String> = self
            .rooms
            .values()
            .filter(|room| room.last_used_at < cutoff)
            .map(|room| room.token.clone())
            .collect();
        for token in &abandoned {
            if let Some(room) = self.rooms.remove(token) {
                self.by_code.remove(&room.code);
                self.by_display.remove(&room.display_token);
                // Only if the teacher has not opened a newer room since.
                if self.by_teacher.get(&room.teacher) == Some(&room.token) {
                    self.by_teacher.remove(&room.teacher);
                }
            }
        }

        // A room may outlive the question it was asking.
        let questions = &self.questions;
        for room in self.rooms.values_mut() {
            if room
                .current
                .as_ref()
                .is_some_and(|id| !questions.contains_key(id))
            {
                room.current = None;
            }
        }

        expired.len()
    }
}

/// Shared server state. Cheap to clone, one instance per process.
#[derive(Clone)]
pub struct AppState(Arc<Inner>);

pub struct Inner {
    pub cfg: Config,
    pub oidc: Oidc,
    /// Key for signing session cookies. Regenerated on every start, which
    /// invalidates old sessions - the price of having no state on disc.
    pub cookie_key: Key,
    pub store: RwLock<Store>,
    pub sessions: RwLock<HashMap<String, Session>>,
    pub logins: RwLock<HashMap<String, PendingLogin>>,
    pub metrics: Metrics,
}

impl Deref for AppState {
    type Target = Inner;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Lets `SignedCookieJar` find the signing key.
impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Self {
        state.cookie_key.clone()
    }
}

impl AppState {
    pub fn new(cfg: Config, oidc: Oidc) -> Self {
        Self(Arc::new(Inner {
            cfg,
            oidc,
            cookie_key: Key::generate(),
            store: RwLock::new(Store::default()),
            sessions: RwLock::new(HashMap::new()),
            logins: RwLock::new(HashMap::new()),
            metrics: Metrics::default(),
        }))
    }

    /// Registers a question asked by `teacher` and returns its id.
    ///
    /// The question goes into the teacher's room, which is opened on their
    /// first question and then reused: the students who are already there see
    /// the new question without having to scan or type anything again.
    pub async fn add_question(
        &self,
        source_name: String,
        content: quiz_common::Question,
        view: StudentView,
        teacher: &str,
    ) -> Result<String> {
        let now = Utc::now();
        let id = random_token();
        let mut store = self.store.write().await;

        let room = match store.room_of_teacher(teacher) {
            Some(room) => room.token.clone(),
            None => {
                let token = random_token();
                let display_token = random_token();
                let code = store.free_code();
                store.insert_room(Room {
                    url: self.cfg.url(&format!("r/{token}"))?.to_string(),
                    display_url: self.cfg.url(&format!("d/{display_token}"))?.to_string(),
                    display_token,
                    presenters: Arc::new(()),
                    students: Presence::default(),
                    token: token.clone(),
                    code,
                    teacher: teacher.to_owned(),
                    current: None,
                    asked: 0,
                    created_at: now,
                    last_used_at: now,
                    updates: broadcast::channel(16).0,
                });
                token
            }
        };
        // These are the room's, and the CLI and the templates show them as
        // the way to reach this question. Whether the presentation view is
        // open has to be known before the room is told about the question,
        // because that makes the view reload.
        let (code, answer_url, display_url, presented, number) = {
            let room = store.room_mut(&room).expect("the room was just created");
            room.asked += 1;
            (
                room.code.clone(),
                room.url.clone(),
                room.display_url.clone(),
                room.is_presented(),
                room.asked,
            )
        };

        store.insert_question(Question {
            display_url,
            presented,
            id: id.clone(),
            room: room.clone(),
            code,
            answer_url,
            source_name,
            number,
            content,
            view,
            teacher: teacher.to_owned(),
            created_at: now,
            closed_at: None,
            revealed: false,
            responses: BTreeMap::new(),
            updates: broadcast::channel(16).0,
        });
        store.trim_room(&room, MAX_ROOM_QUESTIONS);

        let room = store.room_mut(&room).expect("the room is there");
        room.current = Some(id.clone());
        room.last_used_at = now;
        // Everybody in the room moves on to the new question.
        room.notify();
        self.metrics.question_asked();

        Ok(id)
    }

    /// Remembers a login that is under way. Returns `false` when too many
    /// are in progress, in which case the visitor has to try again later.
    pub async fn begin_login(&self, csrf: String, pending: PendingLogin) -> bool {
        insert_pending_login(&mut *self.logins.write().await, csrf, pending)
    }

    /// Closes the room of `teacher`, as `quiz logout` asks the server to do
    /// at the end of a lecture. Returns whether there was one, and how many
    /// questions were dropped with it.
    pub async fn close_room(&self, teacher: &str) -> (bool, usize) {
        match self.store.write().await.remove_room_of_teacher(teacher) {
            Some(questions) => (true, questions),
            None => (false, 0),
        }
    }

    /// The room of `teacher` and the questions kept in it, for `quiz status`.
    pub async fn room_status(&self, teacher: &str) -> Option<RoomInfo> {
        self.store
            .read()
            .await
            .room_status(teacher, self.question_ttl())
    }

    /// How long questions, and rooms nobody asks anything in, are kept.
    pub(crate) fn question_ttl(&self) -> TimeDelta {
        TimeDelta::try_hours(self.cfg.question_ttl_hours).unwrap_or(TimeDelta::zero())
    }

    /// Creates a session for `identity` and returns the session id.
    pub async fn start_session(&self, identity: Identity) -> String {
        let id = random_token();
        let expires_at = Utc::now() + self.session_ttl();
        self.sessions.write().await.insert(
            id.clone(),
            Session {
                identity,
                expires_at,
            },
        );
        id
    }

    /// Returns the identity of a session that is still valid.
    pub async fn session(&self, id: &str) -> Option<Identity> {
        let sessions = self.sessions.read().await;
        let session = sessions.get(id)?;
        (session.expires_at > Utc::now()).then(|| session.identity.clone())
    }

    pub async fn end_session(&self, id: &str) {
        self.sessions.write().await.remove(id);
    }

    fn session_ttl(&self) -> TimeDelta {
        TimeDelta::try_hours(self.cfg.session_ttl_hours).unwrap_or(TimeDelta::zero())
    }

    /// Forgets expired sessions, abandoned logins and old questions.
    pub async fn prune(&self) {
        let now = Utc::now();
        let sessions = {
            let mut sessions = self.sessions.write().await;
            let before = sessions.len();
            sessions.retain(|_, s| s.expires_at > now);
            before - sessions.len()
        };
        let logins = {
            let cutoff = now - login_ttl();
            let mut logins = self.logins.write().await;
            let before = logins.len();
            logins.retain(|_, l| l.created_at > cutoff);
            before - logins.len()
        };
        let questions = self.store.write().await.prune(now - self.question_ttl());
        if sessions + logins + questions > 0 {
            tracing::info!(
                sessions,
                logins,
                questions,
                "dropped expired state from memory"
            );
        }
    }
}

/// An unguessable identifier: 32 hexadecimal characters of a random UUID.
pub fn random_token() -> String {
    Uuid::new_v4().simple().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(created_at: DateTime<Utc>) -> PendingLogin {
        PendingLogin {
            pkce_verifier: PkceCodeVerifier::new("verifier".to_owned()),
            nonce: Nonce::new("nonce".to_owned()),
            next: "/".to_owned(),
            created_at,
        }
    }

    #[test]
    fn logins_that_are_never_finished_cannot_fill_the_memory() {
        let now = Utc::now();
        let mut logins = HashMap::new();
        for i in 0..MAX_PENDING_LOGINS {
            assert!(insert_pending_login(
                &mut logins,
                i.to_string(),
                pending(now)
            ));
        }
        assert_eq!(logins.len(), MAX_PENDING_LOGINS);

        // The table is full of logins that are still fresh, so no more fit.
        assert!(!insert_pending_login(
            &mut logins,
            "one more".to_owned(),
            pending(now)
        ));
        assert_eq!(logins.len(), MAX_PENDING_LOGINS);

        // Once they are old enough to be useless, they make room again.
        let later = now + login_ttl() + TimeDelta::seconds(1);
        assert!(insert_pending_login(
            &mut logins,
            "later".to_owned(),
            pending(later)
        ));
        assert_eq!(logins.len(), 1);
    }

    fn room(token: &str, now: DateTime<Utc>) -> Room {
        Room {
            token: token.to_owned(),
            code: "123456".to_owned(),
            teacher: "teacher".to_owned(),
            url: String::new(),
            display_token: random_token(),
            display_url: String::new(),
            presenters: Arc::new(()),
            students: Presence::default(),
            current: None,
            asked: 0,
            created_at: now,
            last_used_at: now,
            updates: broadcast::channel(16).0,
        }
    }

    fn question(room: &str, number: usize, now: DateTime<Utc>) -> Question {
        Question {
            id: format!("q{number}"),
            room: room.to_owned(),
            code: "123456".to_owned(),
            source_name: "q.md".to_owned(),
            number,
            content: quiz_common::parse_question("Q?\n\n---\n\n- a\n- b\n").unwrap(),
            view: StudentView::default(),
            answer_url: String::new(),
            display_url: String::new(),
            presented: false,
            teacher: "teacher".to_owned(),
            created_at: now,
            closed_at: None,
            revealed: false,
            responses: BTreeMap::new(),
            updates: broadcast::channel(16).0,
        }
    }

    #[test]
    fn a_room_keeps_only_its_newest_questions() {
        let now = Utc::now();
        let mut store = Store::default();
        store.insert_room(room("room", now));
        store.insert_room(room("other", now));
        store.insert_question(question("other", 1000, now));
        for number in 1..=MAX_ROOM_QUESTIONS + 5 {
            store.insert_question(question("room", number, now));
            store.room_mut("room").unwrap().current = Some(format!("q{number}"));
            store.trim_room("room", MAX_ROOM_QUESTIONS);
        }

        let mut kept: Vec<usize> = store
            .questions
            .values()
            .filter(|question| question.room == "room")
            .map(|question| question.number)
            .collect();
        kept.sort_unstable();
        assert_eq!(kept, (6..=MAX_ROOM_QUESTIONS + 5).collect::<Vec<_>>());
        let current = format!("q{}", MAX_ROOM_QUESTIONS + 5);
        assert_eq!(store.room("room").unwrap().current, Some(current));
        // Other rooms are left alone.
        assert!(store.get("q1000").is_some());
    }
}
