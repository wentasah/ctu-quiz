// The command line of `quiz`, and nothing else.
//
// It is a file of its own because `build.rs` includes it verbatim to
// generate the shell completion scripts. Everything here therefore has to
// make sense outside the crate as well: no `crate::`, no other modules, and
// no inner doc comments, which an `include!` does not allow.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use url::Url;

/// Ask students a question and collect their answers.
#[derive(Debug, Parser)]
#[command(name = "quiz", version, about, long_about = None)]
pub struct Cli {
    /// Base URL of the quiz server.
    #[arg(
        long,
        short = 's',
        env = "QUIZ_SERVER",
        default_value = "http://localhost:8080",
        global = true
    )]
    pub server: Url,

    /// Never try to open a browser, just print the URLs.
    #[arg(long, global = true)]
    pub no_browser: bool,

    /// Port the OIDC login listens on; 0 picks a free one. Ignored when the
    /// server prescribes a redirect URI, whose port is used instead.
    #[arg(long, env = "QUIZ_LOGIN_PORT", default_value_t = 0, global = true)]
    pub login_port: u16,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Ask a question, show it to students and save their answers.
    Ask {
        /// Markdown file with the question.
        file: PathBuf,
        /// Where to write the answers; defaults to the question file name
        /// followed by the date and time.
        #[arg(long, short)]
        output: Option<PathBuf>,
        /// Close the question automatically after this many seconds.
        #[arg(long, value_name = "SECONDS")]
        close_after: Option<u64>,
        /// Keep the question text off the students' own screens.
        ///
        /// They read it from the projector and only tap an answer on their
        /// phone, which makes it a lot more work to paste the question into
        /// a chatbot.
        #[arg(long)]
        hide_question: bool,
        /// Keep the answers off the students' own screens as well, leaving
        /// them the bare labels `A`, `B`, `C`, ... to answer with.
        #[arg(long)]
        hide_answers: bool,
        /// Show nothing but the answer labels: the same as passing both
        /// `--hide-question` and `--hide-answers`.
        #[arg(long)]
        labels_only: bool,
        /// Put the answers in a random order.
        ///
        /// Everybody sees the same order - the projector, every phone and
        /// the CSV - but asking the same file again shuffles anew, so the
        /// right answer is not in the same place as last year.
        #[arg(long)]
        shuffle: bool,
    },
    /// Show how a Markdown file is parsed, without contacting the server.
    Parse {
        /// Markdown file with the question.
        file: PathBuf,
    },
    /// Log in to the quiz server and remember the token.
    Login,
    /// Close the room students are in and forget the remembered token.
    Logout {
        /// Log out of all servers.
        #[arg(long)]
        all: bool,
    },
    /// Show who this tool is logged in as.
    Whoami,
    /// Close a question and save its answers, or close the whole room.
    Close {
        /// Question id, as printed by `quiz ask`.
        ///
        /// Without it the room is closed instead: the page students have
        /// open stops working, the questions asked in it are removed from
        /// the server and the next question starts a room with a new code.
        /// The teacher stays logged in.
        id: Option<String>,
        /// Where to write the answers of the question being closed.
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Mark the right answers in the results of a closed question.
    ///
    /// `quiz ask` offers this itself once the question is closed; this is
    /// for when it was interrupted before.
    Reveal {
        /// Question id, as printed by `quiz ask`.
        id: String,
    },
}
