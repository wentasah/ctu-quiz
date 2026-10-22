//! Carrying the rooms, questions and answers over a restart of the server.
//!
//! With `--state-file`, the server writes what is in [`Store`] to that file
//! when it is asked to terminate, and reads it back when it starts, so that
//! restarting it for an update does not cost a lecture its room and the
//! answers collected so far. Browser sessions are not saved: the cookie
//! signing key is new after every start, so the students log in again.
//!
//! Only what cannot be recreated is saved. URLs are built anew from the
//! current `--public-url`, and the channels and the presence of students in
//! the rooms start empty, as the browsers reconnect anyway.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use quiz_common::api::StudentView;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::{
    config::Config,
    state::{AppState, Presence, Question, Response, Room, Store},
};

/// Version of the file format, bumped when an older server could no longer
/// read what a newer one writes, or the other way round.
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Saved {
    version: u32,
    saved_at: DateTime<Utc>,
    rooms: Vec<SavedRoom>,
    questions: Vec<SavedQuestion>,
}

#[derive(Serialize, Deserialize)]
struct SavedRoom {
    token: String,
    code: String,
    teacher: String,
    current: Option<String>,
    asked: usize,
    created_at: DateTime<Utc>,
    last_used_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize)]
struct SavedQuestion {
    id: String,
    room: String,
    source_name: String,
    number: usize,
    content: quiz_common::Question,
    view: StudentView,
    presented: bool,
    teacher: String,
    created_at: DateTime<Utc>,
    /// These two are missing in the files of older servers.
    #[serde(default)]
    close_after: Option<u64>,
    #[serde(default)]
    closes_at: Option<DateTime<Utc>>,
    closed_at: Option<DateTime<Utc>>,
    revealed: bool,
    responses: BTreeMap<String, Response>,
}

impl Saved {
    fn from_store(store: &Store) -> Self {
        let rooms = store
            .rooms()
            .map(|room| SavedRoom {
                token: room.token.clone(),
                code: room.code.clone(),
                teacher: room.teacher.clone(),
                current: room.current.clone(),
                asked: room.asked,
                created_at: room.created_at,
                last_used_at: room.last_used_at,
            })
            .collect();
        let questions = store
            .questions()
            .map(|question| SavedQuestion {
                id: question.id.clone(),
                room: question.room.clone(),
                source_name: question.source_name.clone(),
                number: question.number,
                content: question.content.clone(),
                view: question.view,
                presented: question.presented,
                teacher: question.teacher.clone(),
                created_at: question.created_at,
                close_after: question.close_after,
                closes_at: question.closes_at,
                closed_at: question.closed_at,
                revealed: question.revealed,
                responses: question.responses.clone(),
            })
            .collect();
        Self {
            version: VERSION,
            saved_at: Utc::now(),
            rooms,
            questions,
        }
    }

    /// Puts the saved rooms and questions into `store`. Questions whose room
    /// is not there are dropped.
    fn restore(self, cfg: &Config, store: &mut Store) -> Result<()> {
        for room in self.rooms {
            store.insert_room(Room {
                url: cfg.url(&format!("r/{}", room.token))?.to_string(),
                display_url: cfg.url("d")?.to_string(),
                token: room.token,
                code: room.code,
                teacher: room.teacher,
                presenters: Arc::new(()),
                students: Presence::default(),
                current: room.current,
                asked: room.asked,
                created_at: room.created_at,
                last_used_at: room.last_used_at,
                updates: broadcast::channel(16).0,
            });
        }
        for question in self.questions {
            let Some(room) = store.room(&question.room) else {
                continue;
            };
            let (code, answer_url, display_url) = (
                room.code.clone(),
                room.url.clone(),
                room.display_url.clone(),
            );
            store.insert_question(Question {
                id: question.id,
                room: question.room,
                code,
                source_name: question.source_name,
                number: question.number,
                content: question.content,
                view: question.view,
                answer_url,
                display_url,
                presented: question.presented,
                teacher: question.teacher,
                created_at: question.created_at,
                close_after: question.close_after,
                closes_at: question.closes_at,
                closed_at: question.closed_at,
                revealed: question.revealed,
                responses: question.responses,
                updates: broadcast::channel(16).0,
            });
        }
        Ok(())
    }
}

impl AppState {
    /// Reads the state saved by the previous run from `--state-file`, if
    /// there is one, and drops what has expired in the meantime.
    ///
    /// A file that cannot be read is no reason not to start: the server
    /// warns and starts from scratch, as it would without the file.
    pub async fn load_state(&self) {
        let Some(path) = &self.cfg.state_file else {
            return;
        };
        let saved = match read_saved(path) {
            Ok(Some(saved)) => saved,
            Ok(None) => {
                tracing::info!(path = %path.display(), "no saved state to read");
                return;
            }
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %format!("{error:#}"),
                    "cannot read the saved state, starting from scratch"
                );
                return;
            }
        };

        // Restored aside, so that a failure leaves nothing half restored.
        let saved_at = saved.saved_at;
        let mut restored = Store::default();
        if let Err(error) = saved.restore(&self.cfg, &mut restored) {
            tracing::warn!(
                path = %path.display(),
                error = %format!("{error:#}"),
                "cannot restore the saved state, starting from scratch"
            );
            return;
        }
        let dropped = restored.prune(Utc::now() - self.question_ttl());
        tracing::info!(
            path = %path.display(),
            %saved_at,
            rooms = restored.room_count(),
            questions = restored.questions().count(),
            dropped,
            "state of the previous run restored"
        );
        // The countdowns that were running go on; those that ran out while
        // the server was down close the question right away.
        let countdowns: Vec<_> = restored
            .questions()
            .filter(|question| question.is_open())
            .filter_map(|question| Some((question.id.clone(), question.closes_at?)))
            .collect();
        *self.store.write().await = restored;
        for (id, at) in countdowns {
            self.close_at(id, at);
        }
    }

    /// Writes the current state to `--state-file`, if one is given.
    pub async fn save_state(&self) -> Result<()> {
        let Some(path) = &self.cfg.state_file else {
            return Ok(());
        };
        let (json, rooms, questions) = {
            let store = self.store.read().await;
            let saved = Saved::from_store(&store);
            let counts = (saved.rooms.len(), saved.questions.len());
            (serde_json::to_vec(&saved)?, counts.0, counts.1)
        };
        write_atomically(path, &json)
            .with_context(|| format!("cannot write the state file {}", path.display()))?;
        tracing::info!(path = %path.display(), rooms, questions, "state saved");
        Ok(())
    }
}

/// Reads the state file, or returns `None` when there is none.
fn read_saved(path: &Path) -> Result<Option<Saved>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("cannot read the file"),
    };
    let saved: Saved = serde_json::from_str(&text).context("cannot parse the file")?;
    if saved.version != VERSION {
        anyhow::bail!(
            "the file has version {}, this server reads version {VERSION}",
            saved.version
        );
    }
    Ok(Some(saved))
}

/// Writes `contents` to a file next to `path` and renames it over `path`, so
/// that a crash in the middle never leaves half a file behind.
///
/// Blocking, but the file is small and only written once, on shutdown.
fn write_atomically(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut temporary = PathBuf::from(path);
    temporary.as_mut_os_string().push(".tmp");
    std::fs::write(&temporary, contents)?;
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;
    use clap::Parser;

    use super::*;

    fn config() -> Config {
        Config::parse_from([
            "quiz-server",
            "--oidc-issuer",
            "https://id.example.org",
            "--oidc-client-id",
            "quiz",
            "--public-url",
            "https://quiz.example.org/new",
        ])
        .normalize()
        .unwrap()
    }

    fn room(token: &str, teacher: &str, code: &str, at: DateTime<Utc>) -> SavedRoom {
        SavedRoom {
            token: token.to_owned(),
            code: code.to_owned(),
            teacher: teacher.to_owned(),
            current: Some(format!("{token}-q")),
            asked: 1,
            created_at: at,
            last_used_at: at,
        }
    }

    fn question(room: &str, at: DateTime<Utc>) -> SavedQuestion {
        SavedQuestion {
            id: format!("{room}-q"),
            room: room.to_owned(),
            source_name: "q.md".to_owned(),
            number: 1,
            content: quiz_common::parse_question("Q?\n\n---\n\n- a\n- b\n").unwrap(),
            view: StudentView::default(),
            presented: false,
            teacher: "teacher".to_owned(),
            created_at: at,
            close_after: None,
            closes_at: None,
            closed_at: None,
            revealed: false,
            responses: BTreeMap::from([(
                "student".to_owned(),
                Response {
                    name: "Student".to_owned(),
                    index: 1,
                    answered_at: at,
                },
            )]),
        }
    }

    #[test]
    fn rooms_questions_and_answers_survive_a_round_trip() {
        let now = Utc::now();
        let cfg = config();
        let saved = Saved {
            version: VERSION,
            saved_at: now,
            rooms: vec![room("room", "teacher", "123456", now)],
            questions: vec![question("room", now), question("gone", now)],
        };
        let json = serde_json::to_string(&saved).unwrap();

        let mut store = Store::default();
        serde_json::from_str::<Saved>(&json)
            .unwrap()
            .restore(&cfg, &mut store)
            .unwrap();

        let room = store.room_by_code("123456").unwrap();
        assert_eq!(room.url, "https://quiz.example.org/new/r/room");
        assert_eq!(room.display_url, "https://quiz.example.org/new/d");
        assert!(store.room_of_teacher("teacher").is_some());
        let question = store.current_question("room").unwrap();
        assert_eq!(question.answer_url, room.url);
        assert_eq!(question.responses["student"].index, 1);
        // The question of a room that was not saved is dropped.
        assert!(store.get("gone-q").is_none());

        // Saving again gives the same rooms and questions.
        let again = Saved::from_store(&store);
        assert_eq!(again.rooms.len(), 1);
        assert_eq!(again.questions.len(), 1);
    }

    #[test]
    fn old_state_is_pruned_after_loading() {
        let now = Utc::now();
        let old = now - TimeDelta::try_days(2).unwrap();
        let saved = Saved {
            version: VERSION,
            saved_at: now,
            rooms: vec![
                room("old", "a", "111111", old),
                room("new", "b", "222222", now),
            ],
            questions: vec![question("old", old), question("new", now)],
        };
        let mut store = Store::default();
        saved.restore(&config(), &mut store).unwrap();
        assert_eq!(store.prune(now - TimeDelta::try_hours(12).unwrap()), 1);
        assert!(store.room("old").is_none());
        assert!(store.room_by_code("111111").is_none());
        assert!(store.current_question("new").is_some());
    }
}
