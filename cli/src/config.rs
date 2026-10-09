//! Default values of the command line options, read from configuration files.
//!
//! Two files are read, both in TOML:
//!
//! * `$XDG_CONFIG_HOME/quiz/config.toml`, the teacher's own defaults;
//! * `quiz.toml` in the current directory or the nearest of its parents,
//!   the defaults of one course, kept next to its questions.
//!
//! The nearer file wins over the one in the home directory. A key is the long
//! name of an option, and the options of a subcommand go to a table named
//! after it:
//!
//! ```toml
//! server = "https://quiz.example.org"
//!
//! [ask]
//! shuffle = true
//! close-after = 120
//! ```
//!
//! The values only replace the built-in defaults: an environment variable or
//! an option given on the command line still wins.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use figment::{
    Figment, Profile, Provider,
    providers::{Format, Toml},
    value::{Dict, Value},
};

/// Searched for in the current directory and all its parents.
const PROJECT_FILE: &str = "quiz.toml";

/// `$XDG_CONFIG_HOME/quiz`, where the tool keeps its files.
pub fn dir() -> Result<PathBuf> {
    let dir = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(
            std::env::var_os("HOME").context("neither XDG_CONFIG_HOME nor HOME is set")?,
        )
        .join(".config"),
    };
    Ok(dir.join("quiz"))
}

/// Gives the options of `command` the defaults found in the configuration
/// files. Files that do not exist are skipped.
pub fn apply(command: clap::Command) -> Result<clap::Command> {
    let mut files = Vec::new();
    if let Ok(dir) = dir() {
        files.push(Toml::file(dir.join("config.toml")));
    }
    // A missing file is skipped. A relative path makes figment look in the
    // parent directories too.
    files.push(Toml::file(PROJECT_FILE));

    let mut figment = Figment::new();
    for file in files {
        // The errors of figment do not say which file is broken.
        if let Err(error) = file.data() {
            match file.metadata().source {
                Some(source) => bail!("{source}: {error}"),
                None => bail!(error),
            }
        }
        figment = figment.merge(file);
    }
    with_defaults(&figment, command)
}

/// Gives the options of `command` the defaults that `figment` holds.
fn with_defaults(figment: &Figment, command: clap::Command) -> Result<clap::Command> {
    let mut data = figment.data()?;
    let values = data.remove(&Profile::Default).unwrap_or_default();
    set_defaults(figment, command, values, "")
}

/// Turns the keys of one table into defaults of the options of `command`;
/// `prefix` is the path of the table, for error messages.
fn set_defaults(
    figment: &Figment,
    mut command: clap::Command,
    values: Dict,
    prefix: &str,
) -> Result<clap::Command> {
    for (key, value) in values {
        let source = match figment
            .get_metadata(value.tag())
            .and_then(|m| m.source.as_ref())
        {
            Some(source) => format!(" in {source}"),
            None => String::new(),
        };
        let path = format!("{prefix}{key}");

        if let Value::Dict(_, table) = value {
            let sub = command
                .find_subcommand(&key)
                .cloned()
                .with_context(|| format!("unknown subcommand `{path}`{source}"))?;
            let sub = set_defaults(figment, sub, table, &format!("{path}."))?;
            command = command.mut_subcommand(&key, |_| sub);
            continue;
        }

        let Some(id) = command
            .get_arguments()
            .find(|arg| arg.get_long() == Some(key.as_str()))
            .map(|arg| arg.get_id().clone())
        else {
            bail!("unknown option `{path}`{source}");
        };
        let text = match &value {
            Value::String(_, text) => text.clone(),
            Value::Char(_, char) => char.to_string(),
            Value::Bool(_, flag) => flag.to_string(),
            Value::Num(_, num) => match num.to_i128() {
                Some(int) => int.to_string(),
                None => num.to_f64().unwrap_or_default().to_string(),
            },
            _ => bail!("`{path}`{source} must be a string, a number or a boolean"),
        };
        command = command.mut_arg(id, |arg| arg.default_value(text));
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, FromArgMatches};
    use figment::{
        Figment,
        providers::{Format, Toml},
    };

    use super::with_defaults;
    use crate::cli::{Cli, Command};

    fn parse(toml: &str, args: &[&str]) -> Cli {
        let figment = Figment::new().merge(Toml::string(toml));
        let command = with_defaults(&figment, Cli::command()).unwrap();
        let matches = command.try_get_matches_from(args).unwrap();
        Cli::from_arg_matches(&matches).unwrap()
    }

    #[test]
    fn the_file_replaces_the_built_in_defaults() {
        let toml = "server = 'https://quiz.example.org'\n\
                    [ask]\nshuffle = true\nclose-after = 120\n";
        let cli = parse(toml, &["quiz", "ask", "q.md"]);
        assert_eq!(cli.server.as_str(), "https://quiz.example.org/");
        let Command::Ask {
            shuffle,
            close_after,
            hide_question,
            ..
        } = cli.command
        else {
            panic!("not ask: {:?}", cli.command);
        };
        assert!(shuffle);
        assert_eq!(close_after, Some(120));
        assert!(!hide_question);
    }

    #[test]
    fn the_command_line_wins_over_the_file() {
        let toml = "server = 'https://quiz.example.org'\n[ask]\nclose-after = 120\n";
        let cli = parse(
            toml,
            &[
                "quiz",
                "-s",
                "http://other",
                "ask",
                "--close-after",
                "5",
                "q.md",
            ],
        );
        assert_eq!(cli.server.as_str(), "http://other/");
        assert!(matches!(
            cli.command,
            Command::Ask {
                close_after: Some(5),
                ..
            }
        ));
    }

    #[test]
    fn mistakes_in_the_file_are_reported() {
        for toml in [
            "sever = 'x'",
            "[aks]\nshuffle = true",
            "[ask]\nfile = 'q.md'",
            "server = [1]",
        ] {
            let figment = Figment::new().merge(Toml::string(toml));
            assert!(with_defaults(&figment, Cli::command()).is_err(), "{toml}");
        }
    }
}
