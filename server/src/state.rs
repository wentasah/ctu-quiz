//! In-memory state of the server.
//!
//! The assignment forbids on-disc state, so questions, collected answers and
//! browser sessions live in this process only and are lost on restart. The log
//! file is written but never read back.

use std::{
    collections::{BTreeMap, HashMap},
    ops::Deref,
    sync::Arc,
};

use anyhow::Result;
use axum::extract::FromRef;
use axum_extra::extract::cookie::Key;
use chrono::{DateTime, TimeDelta, Utc};
use openidconnect::{Nonce, PkceCodeVerifier};
use quiz_common::api::{AnswerInfo, LiveState, QuestionInfo, ResultRow, Results};
use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

use crate::{
    auth::{Identity, Oidc},
    config::Config,
};

/// How long a login that was started may still be finished.
const LOGIN_TTL_MINUTES: i64 = 15;

/// How many logins may be in progress at once.
///
/// `GET /login` needs no authentication and remembers something for every
/// visit, so without a limit anybody could fill the server's memory by asking
/// for logins they never finish.
pub const MAX_PENDING_LOGINS: usize = 4096;

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
#[derive(Debug, Clone)]
pub struct Response {
    pub name: String,
    pub index: usize,
    pub answered_at: DateTime<Utc>,
}

/// A question asked by a teacher.
pub struct Question {
    pub id: String,
    /// Short code students type on the main page.
    pub code: String,
    /// Secret part of the presentation URL.
    pub display_token: String,
    /// Name of the Markdown file the question came from.
    pub source_name: String,
    pub content: quiz_common::Question,
    pub answer_url: String,
    pub display_url: String,
    pub teacher: String,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    /// Answers, keyed by login, so that answering twice replaces the answer.
    pub responses: BTreeMap<String, Response>,
    /// Notifies the server-sent-event streams that something changed.
    pub updates: broadcast::Sender<()>,
}

impl Question {
    pub fn is_open(&self) -> bool {
        self.closed_at.is_none()
    }

    /// Question title, falling back to the Markdown file name.
    pub fn title(&self) -> &str {
        self.content.title_or(&self.source_name)
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
            code: self.code.clone(),
            title: self.content.title.clone(),
            source_name: self.source_name.clone(),
            answer_url: self.answer_url.clone(),
            display_url: self.display_url.clone(),
            answers: self
                .content
                .answers
                .iter()
                .map(|a| AnswerInfo {
                    label: a.label.clone(),
                    text: a.plain.clone(),
                })
                .collect(),
            open: self.is_open(),
            answer_count: self.responses.len(),
            counts: self.counts(),
            teacher: self.teacher.clone(),
            created_at: self.created_at,
            closed_at: self.closed_at,
        }
    }

    /// State pushed to browsers. Per-answer counts are only revealed once the
    /// question is closed.
    pub fn live_state(&self) -> LiveState {
        let closed = !self.is_open();
        LiveState {
            open: self.is_open(),
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

/// All questions, indexed by their id, code and display token.
#[derive(Default)]
pub struct Store {
    questions: HashMap<String, Question>,
    by_code: HashMap<String, String>,
    by_display: HashMap<String, String>,
}

impl Store {
    pub fn get(&self, id: &str) -> Option<&Question> {
        self.questions.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Question> {
        self.questions.get_mut(id)
    }

    /// Looks up the question a student typed the code of.
    pub fn by_code(&self, code: &str) -> Option<&Question> {
        self.by_code.get(code).and_then(|id| self.questions.get(id))
    }

    /// Looks up the question from the secret part of its presentation URL.
    pub fn by_display_token(&self, token: &str) -> Option<&Question> {
        self.by_display
            .get(token)
            .and_then(|id| self.questions.get(id))
    }

    pub fn insert(&mut self, question: Question) {
        self.by_code
            .insert(question.code.clone(), question.id.clone());
        self.by_display
            .insert(question.display_token.clone(), question.id.clone());
        self.questions.insert(question.id.clone(), question);
    }

    /// A six-digit code that is not in use.
    fn free_code(&self) -> String {
        loop {
            let number = u128::from_be_bytes(*Uuid::new_v4().as_bytes()) % 1_000_000;
            let code = format!("{number:06}");
            if !self.by_code.contains_key(&code) {
                return code;
            }
        }
    }

    /// Drops questions created before `cutoff`. Returns how many were dropped.
    fn prune(&mut self, cutoff: DateTime<Utc>) -> usize {
        let expired: Vec<String> = self
            .questions
            .values()
            .filter(|q| q.created_at < cutoff)
            .map(|q| q.id.clone())
            .collect();
        for id in &expired {
            if let Some(question) = self.questions.remove(id) {
                self.by_code.remove(&question.code);
                self.by_display.remove(&question.display_token);
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
        }))
    }

    /// Registers a question asked by `teacher` and returns its id.
    pub async fn add_question(
        &self,
        source_name: String,
        content: quiz_common::Question,
        teacher: &str,
    ) -> Result<String> {
        let id = random_token();
        let display_token = random_token();
        let mut store = self.store.write().await;
        let question = Question {
            answer_url: self.cfg.url(&format!("q/{id}"))?.to_string(),
            display_url: self.cfg.url(&format!("d/{display_token}"))?.to_string(),
            id: id.clone(),
            code: store.free_code(),
            display_token,
            source_name,
            content,
            teacher: teacher.to_owned(),
            created_at: Utc::now(),
            closed_at: None,
            responses: BTreeMap::new(),
            updates: broadcast::channel(16).0,
        };
        store.insert(question);
        Ok(id)
    }

    /// Remembers a login that is under way. Returns `false` when too many
    /// are in progress, in which case the visitor has to try again later.
    pub async fn begin_login(&self, csrf: String, pending: PendingLogin) -> bool {
        insert_pending_login(&mut *self.logins.write().await, csrf, pending)
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
        let questions = {
            let ttl =
                TimeDelta::try_hours(self.cfg.question_ttl_hours).unwrap_or(TimeDelta::zero());
            self.store.write().await.prune(now - ttl)
        };
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
}
