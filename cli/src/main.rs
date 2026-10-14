//! Teacher's command line tool.
//!
//! `quiz ask question.md` publishes a question on the quiz server, opens the
//! presentation view in a browser, shows the number of answers as they arrive
//! and, once the teacher closes the question, writes the collected answers next
//! to the Markdown file as a CSV.

mod auth;
/// The clap definition, in a file of its own so that `build.rs` can include
/// it and generate the shell completions from it.
mod cli;
mod client;
mod config;
mod report;

use std::{
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use chrono::Local;
use clap::{CommandFactory, FromArgMatches};
use cli::{Cli, Command};
use client::{Api, Login};
use quiz_common::api::{CreateQuestion, StudentView};
use tokio::io::{AsyncBufReadExt, BufReader, Lines, Stdin};
use url::Url;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = match parse_command_line() {
        Ok(cli) => cli,
        Err(error) => {
            eprintln!("quiz: {error:#}");
            return ExitCode::FAILURE;
        }
    };
    match run(&cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("quiz: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Parses the command line, with the defaults from the configuration files
/// in place of the built-in ones.
fn parse_command_line() -> Result<Cli> {
    let command = config::apply(Cli::command()).context("reading the configuration failed")?;
    Ok(Cli::from_arg_matches(&command.get_matches()).unwrap_or_else(|error| error.exit()))
}

async fn run(cli: &Cli) -> Result<()> {
    match &cli.command {
        Command::Ask {
            file,
            output,
            close_after,
            hide_question,
            hide_answers,
            labels_only,
            shuffle,
        } => {
            ask(
                cli,
                file,
                output.clone(),
                *close_after,
                student_view(*hide_question, *hide_answers, *labels_only),
                *shuffle,
            )
            .await
        }
        Command::Parse { file } => parse(file),
        Command::Login => login(cli).await,
        Command::Logout { all } => logout(cli, *all).await,
        Command::Whoami => whoami(cli).await,
        Command::Close { id, output } => match id {
            Some(id) => close(cli, id, output.clone()).await,
            None => close_room(cli, output.is_some()).await,
        },
        Command::Reveal { id } => reveal(cli, id).await,
    }
}

/// What the `--hide-...` flags leave for the students to see.
///
/// `--labels-only` is the short way of saying both of them.
fn student_view(hide_question: bool, hide_answers: bool, labels_only: bool) -> StudentView {
    StudentView {
        show_question: !(hide_question || labels_only),
        show_answers: !(hide_answers || labels_only),
    }
}

/// The main workflow: publish, present, collect, save.
async fn ask(
    cli: &Cli,
    file: &Path,
    output: Option<PathBuf>,
    close_after: Option<u64>,
    view: StudentView,
    shuffle: bool,
) -> Result<()> {
    let markdown = read_question(file)?;
    report::print_question(
        &quiz_common::parse_question(&markdown)
            .with_context(|| format!("{} is not a usable question", file.display()))?,
    );

    let api = connect(cli, false).await?;
    let question = api
        .create_question(&CreateQuestion {
            source_name: file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "question.md".to_owned()),
            markdown,
            view,
            shuffle,
        })
        .await?;

    // The order on the screen is the server's now, so show it rather than
    // the one the file has.
    if shuffle {
        println!("\nThe answers were shuffled into this order:");
        report::print_answers(&question.answers);
    }

    println!("Question code:     {}", question.code);
    println!("Students answer at: {}", question.answer_url);
    println!("Presentation view:  {}", question.display_url);
    println!("Students see:       {}", describe(view));
    if question.presented {
        println!("\nThe presentation view that is already open shows the question.");
    } else {
        show_in_browser(&question.display_url, !cli.no_browser);
    }

    // One reader for the whole run, so that an Enter typed ahead is not
    // lost between the two prompts.
    let mut stdin = Some(BufReader::new(tokio::io::stdin()).lines());
    wait_for_close(&api, &question.id, close_after, &mut stdin).await?;

    let closed = api.close(&question.id).await?;
    let results = api.results(&question.id).await?;

    let path = output.unwrap_or_else(|| report::csv_path(file, Local::now()));
    report::write_csv(&path, &results)?;
    report::print_summary(&closed, &results);
    println!("\nAnswers saved to {}", path.display());

    if closed.answers.iter().any(|answer| answer.points.is_some()) {
        wait_for_reveal(&api, &question.id, &mut stdin).await?;
    }
    Ok(())
}

/// Lets the teacher talk about the results before the right answers are
/// marked in them, on the projector and on the students' screens.
async fn wait_for_reveal(
    api: &Api,
    id: &str,
    stdin: &mut Option<Lines<BufReader<Stdin>>>,
) -> Result<()> {
    let later = format!("`quiz reveal {id}` reveals them later");
    let Some(lines) = stdin else {
        println!("The right answers were not revealed; {later}.");
        return Ok(());
    };
    println!("Press Enter to reveal the right answers (Ctrl-C ends without them).");

    tokio::select! {
        line = lines.next_line() => match line {
            Ok(Some(_)) => {
                api.reveal(id).await?;
                println!("The right answers are revealed.");
            }
            // No terminal on the standard input: nobody to press Enter.
            Ok(None) | Err(_) => println!("The right answers were not revealed; {later}."),
        },
        // A plain `return` would leave the Enter-reading task stuck on
        // tokio's blocking pool, which the runtime waits for on shutdown, so
        // the process would not actually exit until Enter is pressed anyway.
        _ = tokio::signal::ctrl_c() => {
            println!("\nThe right answers were not revealed; {later}.");
            std::process::exit(0);
        }
    }
    Ok(())
}

/// One line about what is on the students' own screens, so that the teacher
/// notices a `--hide-...` they did not mean to pass.
fn describe(view: StudentView) -> &'static str {
    match (view.show_question, view.show_answers) {
        (true, true) => "the question and the answers",
        (true, false) => "the question, and the answer labels only",
        (false, true) => "the answers only; the question is on the projector",
        (false, false) => "the answer labels only; everything else is on the projector",
    }
}

/// Shows the parsed question without publishing it.
fn parse(file: &Path) -> Result<()> {
    let markdown = read_question(file)?;
    let question = quiz_common::parse_question(&markdown)
        .with_context(|| format!("{} is not a usable question", file.display()))?;
    report::print_question(&question);
    println!(
        "{} is a valid {}question with {} possible answers.",
        file.display(),
        if question.is_scored() { "scored " } else { "" },
        question.answers.len()
    );
    Ok(())
}

async fn login(cli: &Cli) -> Result<()> {
    let api = connect(cli, true).await?;
    let me = api.whoami().await?;
    println!("Logged in as {} ({}).", me.name, me.login);
    if !me.is_teacher {
        println!("Note: this login is not allowed to ask questions on this server.");
    }
    Ok(())
}

/// Ends the lecture: the students' page stops working and the token is
/// forgotten.
///
/// The room is closed first, because doing so needs the very token that is
/// about to be dropped. A server that cannot be reached is only reported: the
/// token is forgotten either way, which is what the teacher asked for.
async fn logout(cli: &Cli, all: bool) -> Result<()> {
    let server = cli.server.as_str().to_owned();
    let selected = (!all).then_some(server.as_str());

    for (server, token) in auth::cached(selected) {
        let Ok(url) = Url::parse(&server) else {
            continue;
        };
        let Ok(http) = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
        else {
            continue;
        };
        match Api::new(http, url, token, None).close_room().await {
            Ok(room) if room.closed => println!(
                "Room on {server} closed ({} question(s) removed); students can no longer answer.",
                room.questions
            ),
            Ok(_) => println!("No room was open on {server}."),
            Err(error) => println!("Cannot close the room on {server} ({error:#})."),
        }
    }

    let forgotten = auth::forget(selected)?;
    if forgotten {
        println!("Token forgotten.");
    } else {
        println!("There was no token to forget.");
    }
    Ok(())
}

async fn whoami(cli: &Cli) -> Result<()> {
    let api = connect(cli, false).await?;
    let me = api.whoami().await?;
    println!("{} ({})", me.name, me.login);
    println!(
        "{}",
        if me.is_teacher {
            "May ask questions."
        } else {
            "May not ask questions on this server."
        }
    );
    Ok(())
}

/// Closes a question whose `quiz ask` was interrupted, and saves its answers.
async fn close(cli: &Cli, id: &str, output: Option<PathBuf>) -> Result<()> {
    let api = connect(cli, false).await?;
    let closed = api.close(id).await?;
    let results = api.results(id).await?;

    let path =
        output.unwrap_or_else(|| report::csv_path(Path::new(&closed.source_name), Local::now()));
    report::write_csv(&path, &results)?;
    report::print_summary(&closed, &results);
    println!("\nAnswers saved to {}", path.display());
    Ok(())
}

/// Marks the right answers of a question whose `quiz ask` ended before.
async fn reveal(cli: &Cli, id: &str) -> Result<()> {
    let api = connect(cli, false).await?;
    let question = api.reveal(id).await?;
    if question
        .answers
        .iter()
        .any(|answer| answer.points.is_some())
    {
        println!("The right answers are revealed.");
    } else {
        println!("The question has no points, so there are no right answers to reveal.");
    }
    Ok(())
}

/// Ends the lecture without logging out: the room, and everything asked in
/// it, is dropped.
async fn close_room(cli: &Cli, has_output: bool) -> Result<()> {
    if has_output {
        bail!("--output needs the id of the question whose answers to save");
    }

    let api = connect(cli, false).await?;
    let room = api.close_room().await?;
    if room.closed {
        println!(
            "Room closed ({} question(s) removed); students can no longer answer.",
            room.questions
        );
        println!("The next question will open a room with a new code.");
    } else {
        println!("There was no room to close.");
    }
    Ok(())
}

/// Logs in if needed and returns a client for the server API.
async fn connect(cli: &Cli, force_login: bool) -> Result<Api> {
    // Redirects are not followed: the API answers directly, and the OIDC
    // discovery must not be sent anywhere else.
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("building the HTTP client failed")?;

    let info = Api::oidc_info(&http, &cli.server).await?;
    let token = auth::token(
        &http,
        cli.server.as_str(),
        &info,
        cli.login_port,
        !cli.no_browser,
        force_login,
    )
    .await?;

    let login = Login {
        info,
        port: cli.login_port,
        open_browser: !cli.no_browser,
    };
    Ok(Api::new(http, cli.server.clone(), token, Some(login)))
}

/// Reads the Markdown of a question.
///
/// A question file may be made executable and given a shebang line, so that
/// asking the question is a matter of running the file:
///
/// ```text
/// #!/usr/bin/env -S quiz ask
/// ```
///
/// Such a line is part of the script, not of the question, so it is dropped.
/// Only an executable file is treated this way; in an ordinary Markdown file a
/// first line starting with `#!` stays part of the question.
fn read_question(file: &Path) -> Result<String> {
    let markdown =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    if is_executable(file) {
        return Ok(strip_shebang(&markdown).to_owned());
    }
    Ok(markdown)
}

/// Whether the file has any of the execute bits set. On systems without file
/// permissions nothing is executable, so nothing is stripped.
fn is_executable(file: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(file).is_ok_and(|data| data.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        false
    }
}

/// Removes a leading `#!` line, if there is one.
fn strip_shebang(markdown: &str) -> &str {
    let Some(rest) = markdown.strip_prefix("#!") else {
        return markdown;
    };
    match rest.split_once('\n') {
        Some((_, after)) => after,
        // A file that is nothing but a shebang line.
        None => "",
    }
}

/// Requirement 7: show the question in a browser, or print the URL when that
/// is not possible.
fn show_in_browser(url: &str, open_browser: bool) {
    if open_browser {
        match open::that_detached(url) {
            Ok(()) => {
                println!("\nThe presentation view was opened in a browser.");
                return;
            }
            Err(error) => println!("\nCannot open a browser ({error})."),
        }
    }
    println!("\nOpen the presentation view manually:\n\n    {url}\n");
}

/// Shows how many of the students in the room have answered, until the
/// teacher closes the question.
///
/// `stdin` becomes `None` once the standard input turns out to be closed.
async fn wait_for_close(
    api: &Api,
    id: &str,
    close_after: Option<u64>,
    stdin: &mut Option<Lines<BufReader<Stdin>>>,
) -> Result<()> {
    println!("Press Enter to close the question and save the answers (Ctrl-C works too).");

    // Redrawing one line is for a terminal; a log file wants one line per
    // change instead of escape sequences.
    let terminal = std::io::stdout().is_terminal();
    let mut last_counts = None;
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    let deadline =
        close_after.map(|seconds| tokio::time::Instant::now() + Duration::from_secs(seconds));

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let question = api.question(id).await?;
                // Answered out of everybody in the room.
                let counts = (question.answer_count, question.people);
                if terminal {
                    print!("\r  answers: {}/{}\x1b[K", counts.0, counts.1);
                    let _ = std::io::stdout().flush();
                } else if last_counts != Some(counts) {
                    println!("  answers: {}/{}", counts.0, counts.1);
                }
                last_counts = Some(counts);

                if !question.open {
                    println!("\nThe question has been closed.");
                    return Ok(());
                }
                if deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
                    println!();
                    return Ok(());
                }
            }
            // The guard keeps the expression from being evaluated at all
            // once the standard input is gone.
            line = stdin.as_mut().expect("guarded").next_line(), if stdin.is_some() => {
                match line {
                    Ok(Some(_)) => return Ok(()),
                    // No terminal on the standard input: wait for Ctrl-C.
                    Ok(None) | Err(_) => *stdin = None,
                }
            }
            _ = tokio::signal::ctrl_c() => {
                println!();
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{describe, read_question, strip_shebang, student_view};

    #[test]
    fn a_shebang_line_is_removed() {
        assert_eq!(
            strip_shebang("#!/usr/bin/env -S quiz ask\n\nQ?\n"),
            "\nQ?\n"
        );
        assert_eq!(strip_shebang("#!quiz ask"), "");
        // Only the very first line, and only a real shebang.
        assert_eq!(
            strip_shebang("# Heading\n#!not a shebang\n"),
            "# Heading\n#!not a shebang\n"
        );
        assert_eq!(strip_shebang(" #!quiz ask\nQ?\n"), " #!quiz ask\nQ?\n");
    }

    #[test]
    fn hiding_flags_decide_what_students_get_to_see() {
        let all = student_view(false, false, false);
        assert!(all.show_question && all.show_answers);
        assert_eq!(describe(all), "the question and the answers");

        let hidden = student_view(true, false, false);
        assert!(!hidden.show_question && hidden.show_answers);

        // Both flags: the students' screens carry nothing but the labels,
        // and `--labels-only` is the same thing said in one word.
        let both = student_view(true, true, false);
        let labels_only = student_view(false, false, true);
        assert_eq!(both, labels_only);
        assert!(!labels_only.show_question && !labels_only.show_answers);
        assert!(describe(labels_only).contains("labels only"));
    }

    #[cfg(unix)]
    #[test]
    fn the_shebang_of_an_executable_question_is_not_part_of_the_question() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("quiz-shebang-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("otazky.md");
        let source = "#!/usr/bin/env -S quiz ask\n\nQ?\n\n---\n\n- a\n- b\n";
        std::fs::write(&file, source).unwrap();

        // Not executable: the file is a plain Markdown file and is read as is.
        assert_eq!(read_question(&file).unwrap(), source);

        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        let markdown = read_question(&file).unwrap();
        assert!(!markdown.contains("#!"), "{markdown}");
        assert_eq!(
            quiz_common::parse_question(&markdown)
                .unwrap()
                .prompt_markdown,
            "Q?"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
