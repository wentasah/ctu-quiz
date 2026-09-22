//! Types shared between the quiz server and the teacher CLI.
//!
//! The crate has two jobs:
//!
//! * [`question`] parses a question written in Markdown into a prompt and a
//!   list of possible answers (see [`question::parse`]).
//! * [`api`] defines the JSON payloads of the HTTP API that the CLI uses to
//!   talk to the server.

pub mod api;
pub mod question;

pub use api::*;
pub use question::{Answer, ParseError, Question, parse as parse_question};
