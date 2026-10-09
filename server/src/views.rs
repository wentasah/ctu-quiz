//! HTML pages, rendered from the templates in `server/templates`.

use std::sync::OnceLock;

use askama::Template;

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
    /// Whether the answer earns points. Shown with the results only, once
    /// the question is closed.
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
            correct: answer.is_correct(),
        })
        .collect()
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
    /// Title of the question, empty when it is not for students to see.
    pub title: String,
    pub prompt_html: String,
    /// Whether the question text is for the students' own screens.
    pub show_question: bool,
    /// Whether the answers are shown with their text, or as bare labels.
    pub show_answers: bool,
    /// Whether the teacher has yet to ask the next question.
    pub waiting: bool,
    pub open: bool,
    pub answer_count: usize,
    /// Label of the student's own answer, empty if they did not answer.
    pub chosen_label: String,
    pub bars: Vec<Bar>,
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
    /// Secret part of the presentation URL, used to build the event stream URL.
    pub token: String,
    /// Id of the question shown, so that the page notices when the stream
    /// starts talking about another one.
    pub question: String,
    pub title: String,
    pub prompt_html: String,
    pub open: bool,
    pub answer_count: usize,
    pub code: String,
    pub answer_url: String,
    /// Host (and path prefix) students have to type by hand.
    pub host: String,
    pub qr_svg: String,
    pub bars: Vec<Bar>,
}
