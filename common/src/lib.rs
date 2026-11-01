//! Types shared between the quiz server and the teacher CLI.
//!
//! The crate has two jobs:
//!
//! * [`question`] parses a question written in Markdown into a prompt and a
//!   list of possible answers (see [`question::parse`]).
//! * [`api`] defines the JSON payloads of the HTTP API that the CLI uses to
//!   talk to the server.
//! * [`highlight`] renders fenced code blocks, with syntax highlighting when
//!   the `highlight` feature is on.

pub mod api;
pub mod highlight;
pub mod question;

pub use api::*;
pub use question::{
    Answer, Image, Images, ParseError, Question, parse as parse_question, parse_with_images,
};
