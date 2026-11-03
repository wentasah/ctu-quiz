//! Colors in the terminal output.
//!
//! The output goes through [`anstream`]'s `println!`, which strips the
//! escape sequences unless they are wanted: `--color always` and `--color
//! never` say so outright; `auto`, the default, keeps them only when the
//! standard output is a terminal that can show them and neither `NO_COLOR`
//! (see <https://no-color.org>) nor `CLICOLOR=0` says otherwise.
//!
//! The styles are [`anstyle`] ones, written as `{GREEN}text{GREEN:#}`.

use anstyle::{AnsiColor, Style};

/// Headings and the answer labels.
pub const BOLD: Style = Style::new().bold();
/// What is there for the teacher only, and answers worth no points.
pub const DIM: Style = Style::new().dimmed();
/// Correct answers and things that went well.
pub const GREEN: Style = AnsiColor::Green.on_default();
/// Wrong answers.
pub const RED: Style = AnsiColor::Red.on_default();
/// Warnings and what the teacher should not overlook.
pub const YELLOW: Style = AnsiColor::Yellow.on_default();
/// URLs and the bars of the summary.
pub const CYAN: Style = AnsiColor::Cyan.on_default();

/// Decides, once and for all, whether the output is colored.
pub fn init(choice: clap::ColorChoice) {
    match choice {
        clap::ColorChoice::Auto => anstream::ColorChoice::Auto,
        clap::ColorChoice::Always => anstream::ColorChoice::Always,
        clap::ColorChoice::Never => anstream::ColorChoice::Never,
    }
    .write_global();
}
