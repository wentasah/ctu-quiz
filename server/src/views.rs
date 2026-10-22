//! HTML pages, rendered from the templates in `server/templates`.

use std::sync::OnceLock;

use askama::Template;

use chrono::{DateTime, Utc};

use crate::{auth::Identity, state::Question};

/// Path prefix of the server inside its public URL, e.g. `/quiz`, or an empty
/// string when the server is served from the root of a host.
///
/// It is a property of the process, so a global spares every handler from
/// passing it around.
static PREFIX: OnceLock<String> = OnceLock::new();

pub fn set_prefix(prefix: String) {
    let _ = PREFIX.set(prefix);
}

pub fn prefix() -> String {
    PREFIX.get().cloned().unwrap_or_default()
}

/// The stylesheet and the script, compiled into the binary.
pub const CSS: &str = include_str!("../static/app.css");
pub const JS: &str = include_str!("../static/app.js");

/// A short hash of [`CSS`] and [`JS`], appended to their URLs as `?v=...`.
///
/// Browsers cache the two files for a while; a new version of the server
/// changes the URL, so pages never pair new HTML with an old stylesheet.
pub fn asset_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        (CSS, JS).hash(&mut hasher);
        format!("{:08x}", hasher.finish() as u32)
    })
}

/// Builds a link to a path of this server, honouring the path prefix.
pub fn link(path: &str) -> String {
    debug_assert!(path.starts_with('/'));
    format!("{}{path}", prefix())
}

/// One possible answer, together with its share of the collected answers.
///
/// The same struct drives the list of buttons a student sees and the bar graph
/// shown once the question is closed.
pub struct Bar {
    pub label: String,
    /// The answer text as HTML.
    pub html: String,
    pub count: usize,
    /// Width of the bar, in per cent of the most popular answer.
    pub width: u32,
    /// Share of all collected answers, in per cent.
    pub share: u32,
    /// Whether this is (one of) the most popular answer(s).
    pub best: bool,
    /// Whether this is the answer of the student looking at the page.
    pub chosen: bool,
    /// Whether the answer earns points. Known only once the teacher has
    /// revealed it, which needs the question to be closed.
    pub correct: bool,
}

/// Turns the answers of a question into bars.
pub fn bars(question: &Question, chosen: Option<usize>) -> Vec<Bar> {
    let counts = question.counts();
    let total = question.responses.len();
    let max = counts.iter().copied().max().unwrap_or(0);

    let percent = |count: usize, of: usize| -> u32 {
        if of == 0 {
            0
        } else {
            (count as f64 * 100.0 / of as f64).round() as u32
        }
    };

    question
        .content
        .answers
        .iter()
        .zip(&counts)
        .enumerate()
        .map(|(index, (answer, &count))| Bar {
            label: answer.label.clone(),
            html: answer.html.clone(),
            count,
            width: percent(count, max),
            share: percent(count, total),
            best: count > 0 && count == max,
            chosen: chosen == Some(index),
            correct: question.revealed && answer.is_correct(),
        })
        .collect()
}

/// Title of the browser tab, for a page showing `question` or, before the
/// first one, none. It is neither the question's heading, which may be hidden
/// from students, nor the name of the Markdown file.
pub fn page_title(question: Option<&Question>) -> String {
    match question {
        Some(question) => format!("Kvíz – otázka {}", question.number),
        None => "Kvíz".to_owned(),
    }
}

/// The main page, where students type the question code.
#[derive(Template)]
#[template(path = "index.html")]
pub struct IndexPage {
    pub prefix: String,
    pub user: Identity,
    /// Empty unless the previously typed code was wrong.
    pub error: String,
}

/// A short informational or error page.
#[derive(Template)]
#[template(path = "message.html")]
pub struct MessagePage {
    pub prefix: String,
    pub title: String,
    pub message: String,
}

/// The page a student stays on: the room, showing whichever question the
/// teacher is asking, or waiting for the next one.
#[derive(Template)]
#[template(path = "answer.html")]
pub struct AnswerPage {
    pub prefix: String,
    pub user: Identity,
    /// Token of the room, i.e. the page's own URL.
    pub room: String,
    /// The numeric code of the room, shown so that a neighbour can join.
    pub code: String,
    /// Id of the question being shown; empty while there is none.
    pub question: String,
    /// Title of the browser tab.
    pub title: String,
    pub prompt_html: String,
    /// Whether the question text is for the students' own screens.
    pub show_question: bool,
    /// Whether the answers are shown with their text, or as bare labels.
    pub show_answers: bool,
    /// Whether the teacher has yet to ask the next question.
    pub waiting: bool,
    pub open: bool,
    /// Whether the right answers are marked in the results.
    pub revealed: bool,
    pub answer_count: usize,
    /// Label of the student's own answer, empty if they did not answer.
    pub chosen_label: String,
    /// How the student's answer scored, known once the answers are revealed.
    pub verdict: Option<Verdict>,
    pub bars: Vec<Bar>,
}

/// How a student's answer scored, compared to the best answer of the question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Correct,
    Partial,
    Wrong,
}

impl Verdict {
    /// Judges the answer at `index`; `None` for an unscored question.
    pub fn of(question: &quiz_common::Question, index: usize) -> Option<Verdict> {
        let answers = &question.answers;
        let max = answers
            .iter()
            .filter_map(|a| a.points)
            .fold(f64::NEG_INFINITY, f64::max);
        let points = answers.get(index)?.points?;
        Some(if points <= 0.0 {
            Verdict::Wrong
        } else if points < max {
            Verdict::Partial
        } else {
            Verdict::Correct
        })
    }
}

/// The presentation view, shown by the teacher on a projector.
#[derive(Template)]
#[template(path = "display.html")]
pub struct DisplayPage {
    pub prefix: String,
    /// Always true: the presentation view is the one screen that shows
    /// everything. It is here because `results.html` is shared with the
    /// students' page, which may not.
    pub show_answers: bool,
    /// Id of the question shown, so that the page notices when the stream
    /// starts talking about another one.
    pub question: String,
    pub title: String,
    pub prompt_html: String,
    pub open: bool,
    /// Whether the right answers are marked in the results.
    pub revealed: bool,
    pub answer_count: usize,
    pub code: String,
    pub answer_url: String,
    /// Host (and path prefix) students have to type by hand.
    pub host: String,
    pub qr_svg: String,
    pub bars: Vec<Bar>,
    /// Time left until the question closes by itself, with `--close-after`.
    pub countdown: Option<Countdown>,
}

/// How long until a question closes by itself, relative to when the page was
/// rendered, so that a browser with its clock off still counts down right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Countdown {
    pub remaining_ms: i64,
    pub total_ms: i64,
}

impl Countdown {
    /// The countdown of `question` at `now`, if it has started.
    pub fn of(question: &Question, now: DateTime<Utc>) -> Option<Self> {
        let closes_at = question.closes_at?;
        let total_ms = i64::try_from(question.close_after?)
            .ok()?
            .saturating_mul(1000);
        Some(Self {
            remaining_ms: (closes_at - now).num_milliseconds().clamp(0, total_ms),
            total_ms: total_ms.max(1),
        })
    }

    /// Part of the time still left, from 1 down to 0.
    pub fn left(&self) -> f64 {
        self.remaining_ms as f64 / self.total_ms as f64
    }
}

#[cfg(test)]
mod tests {
    use super::Verdict;

    #[test]
    fn verdict_compares_the_points_with_the_best_answer() {
        let question = quiz_common::question::parse(
            "Which one?\n\n---\n\n- best {2}\n- half {1}\n- none {0}\n- bad {-1}\n",
        )
        .unwrap();
        let verdicts: Vec<_> = (0..4).map(|i| Verdict::of(&question, i)).collect();
        assert_eq!(
            verdicts,
            [
                Some(Verdict::Correct),
                Some(Verdict::Partial),
                Some(Verdict::Wrong),
                Some(Verdict::Wrong),
            ]
        );
    }

    #[test]
    fn an_unscored_question_has_no_verdict() {
        let question = quiz_common::question::parse("Which one?\n\n---\n\n- one\n- two\n").unwrap();
        assert_eq!(Verdict::of(&question, 0), None);
    }
}
