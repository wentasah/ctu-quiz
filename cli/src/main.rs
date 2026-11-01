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
use client::{Api, Events, Login};
use quiz_common::api::{CreateQuestion, QuestionInfo, StudentView};
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
            output_dir,
            room_dir,
            close_after,
            hide_question,
            hide_answers,
            labels_only,
            shuffle,
        } => {
            ask(
                cli,
                file,
                Destination {
                    output: output.clone(),
                    dir: output_dir.as_deref(),
                    room_dir: *room_dir,
                },
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
        Command::Status => status(cli).await,
        Command::Close { id, output } => match id {
            Some(id) => close_question(cli, id, output.clone()).await,
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

/// Where `quiz ask` saves the results, as `--output`, `--output-dir` and
/// `--room-dir` say.
struct Destination<'a> {
    output: Option<PathBuf>,
    dir: Option<&'a Path>,
    room_dir: bool,
}

impl Destination<'_> {
    /// The directory of the results of `file`, created if needed, or `None`
    /// to keep them next to it.
    ///
    /// Without a room to name it after, `--room-dir` is left out rather than
    /// losing the answers, which are collected by then.
    async fn dir(&self, api: &Api, file: &Path) -> Result<Option<PathBuf>> {
        let room = if self.room_dir {
            match api.room_status().await {
                Ok(status) => status.room,
                Err(error) => {
                    println!("Cannot find out the room ({error:#}).");
                    None
                }
            }
        } else {
            None
        };
        let dir = match room {
            Some(room) => {
                let opened = room.created_at.with_timezone(&Local);
                Some(report::room_dir_path(file, self.dir, &room.code, opened))
            }
            None => self.dir.map(Path::to_owned),
        };
        if let Some(dir) = &dir {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("cannot create {}", dir.display()))?;
        }
        Ok(dir)
    }
}

/// The main workflow: publish, present, collect, save.
async fn ask(
    cli: &Cli,
    file: &Path,
    destination: Destination<'_>,
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
            close_after,
            start_countdown: cli.no_browser,
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
    if let Some(seconds) = close_after {
        if cli.no_browser {
            println!("The question closes by itself in {seconds} s.");
        } else {
            println!(
                "The question closes by itself {seconds} s after the presentation view shows it."
            );
        }
    }
    wait_for_close(&api, &question.id, &mut stdin).await?;

    let closed = api.close(&question.id).await?;
    let results = api.results(&question.id).await?;

    let dir = destination.dir(&api, file).await?;
    let path = destination
        .output
        .unwrap_or_else(|| report::csv_path(file, dir.as_deref(), Local::now()));
    report::write_csv(&path, &results)?;
    report::print_summary(&closed, &results);
    println!("\nAnswers saved to {}", path.display());
    save_room(&api, file, dir.as_deref()).await;

    if closed.answers.iter().any(|answer| answer.points.is_some()) {
        wait_for_reveal(&api, &question.id, &mut stdin).await?;
    }
    Ok(())
}

/// Lets the teacher talk about the results before the correct answers are
/// marked in them, on the projector and on the students' screens.
async fn wait_for_reveal(
    api: &Api,
    id: &str,
    stdin: &mut Option<Lines<BufReader<Stdin>>>,
) -> Result<()> {
    let later = format!("`quiz reveal {id}` reveals them later");
    let Some(lines) = stdin else {
        println!("The correct answers were not revealed; {later}.");
        return Ok(());
    };
    println!(
        "Press Enter to reveal the correct answers (Ctrl-C ends without them, the button \
         in the presentation view reveals them too)."
    );

    // The teacher may reveal them in the presentation view instead. Losing
    // the event stream only means that Enter is the one way left.
    let mut events = None;
    let mut watching = true;
    loop {
        tokio::select! {
            line = lines.next_line() => {
                match line {
                    Ok(Some(_)) => {
                        api.reveal(id).await?;
                        println!("The correct answers are revealed.");
                    }
                    // No terminal on the standard input: nobody to press Enter.
                    Ok(None) | Err(_) => {
                        println!("The correct answers were not revealed; {later}.");
                    }
                }
                return Ok(());
            }
            question = next_state(api, id, &mut events), if watching => match question {
                Ok(question) if question.revealed => {
                    println!("The correct answers were revealed in the presentation view.");
                    // As below: the Enter-reading task would keep the
                    // process alive.
                    std::process::exit(0);
                }
                Ok(_) => {}
                Err(_) => watching = false,
            },
            // A plain `return` would leave the Enter-reading task stuck on
            // tokio's blocking pool, which the runtime waits for on shutdown, so
            // the process would not actually exit until Enter is pressed anyway.
            _ = tokio::signal::ctrl_c() => {
                println!("\nThe correct answers were not revealed; {later}.");
                std::process::exit(0);
            }
        }
    }
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

/// Shows the room and the questions the server still keeps in it, which is
/// where to find the id of a question whose `quiz ask` was interrupted.
async fn status(cli: &Cli) -> Result<()> {
    let api = connect(cli, false).await?;
    match api.room_status().await?.room {
        Some(room) => report::print_room(&room),
        None => println!("No room is open; the next `quiz ask` opens one."),
    }
    Ok(())
}

/// Closes a question whose `quiz ask` was interrupted, and saves its answers.
async fn close_question(cli: &Cli, id: &str, output: Option<PathBuf>) -> Result<()> {
    let api = connect(cli, false).await?;
    let closed = api.close(id).await?;
    let results = api.results(id).await?;

    let path = output
        .unwrap_or_else(|| report::csv_path(Path::new(&closed.source_name), None, Local::now()));
    report::write_csv(&path, &results)?;
    report::print_summary(&closed, &results);
    println!("\nAnswers saved to {}", path.display());
    save_room(&api, Path::new(&closed.source_name), None).await;
    Ok(())
}

/// Rewrites the CSV with the points of the whole room, in `dir` or next to
/// `source`, so that it is up to date after every question.
///
/// It is built anew from the closed scored questions the server keeps in the
/// room, so closing a question again or out of order does no harm. Failing
/// to write it is only reported: the answers of the question are saved by
/// then, and the teacher may still want to reveal the correct ones.
async fn save_room(api: &Api, source: &Path, dir: Option<&Path>) {
    match write_room(api, source, dir).await {
        Ok(Some(path)) => println!("Points of the room saved to {}", path.display()),
        Ok(None) => {}
        Err(error) => println!("Cannot save the points of the room ({error:#})."),
    }
}

/// Writes the room CSV and says where, or `None` when there is nothing to
/// write: no room, or no closed scored question in it.
async fn write_room(api: &Api, source: &Path, dir: Option<&Path>) -> Result<Option<PathBuf>> {
    let Some(room) = api.room_status().await?.room else {
        return Ok(None);
    };
    let mut questions = Vec::new();
    for question in &room.questions {
        let info = &question.info;
        // Only the question asked last still lists its answers; the points
        // of the earlier ones are in their results.
        let scored = info.answers.iter().any(|answer| answer.points.is_some());
        if info.open || !(scored || info.answers.is_empty()) {
            continue;
        }
        let results = api.results(&info.id).await?;
        if scored || results.rows.iter().any(|row| row.points.is_some()) {
            questions.push(results);
        }
    }
    if questions.is_empty() {
        return Ok(None);
    }

    let opened = room.created_at.with_timezone(&Local);
    let path = report::room_csv_path(source, dir, &room.code, opened);
    report::write_room_csv(&path, &questions)?;
    Ok(Some(path))
}

/// Marks the correct answers of a question whose `quiz ask` ended before.
async fn reveal(cli: &Cli, id: &str) -> Result<()> {
    let api = connect(cli, false).await?;
    let question = api.reveal(id).await?;
    if question
        .answers
        .iter()
        .any(|answer| answer.points.is_some())
    {
        println!("The correct answers are revealed.");
    } else {
        println!("The question has no points, so there are no correct answers to reveal.");
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

/// The next line of the standard input, or never once it is gone.
///
/// A guard on a `select!` branch would not do: it stops the future from being
/// polled, but the expression that makes it is still evaluated.
async fn next_line(stdin: &mut Option<Lines<BufReader<Stdin>>>) -> std::io::Result<Option<String>> {
    match stdin {
        Some(lines) => lines.next_line().await,
        None => std::future::pending().await,
    }
}

/// How long the event stream may stay silent before it is taken for broken.
/// The server sends a keep-alive comment every 15 seconds.
const STREAM_SILENCE: Duration = Duration::from_secs(45);

/// Shows how many of the students in the room have answered, until the
/// teacher closes the question.
///
/// The counts come over server-sent events, so they show up as soon as
/// somebody answers and nothing is sent while nothing happens.
///
/// `stdin` becomes `None` once the standard input turns out to be closed.
async fn wait_for_close(
    api: &Api,
    id: &str,
    stdin: &mut Option<Lines<BufReader<Stdin>>>,
) -> Result<()> {
    println!(
        "Press Enter to close the question and save the answers (Ctrl-C or the button \
         in the presentation view work too)."
    );

    // Redrawing one line is for a terminal; a log file wants one line per
    // change instead of escape sequences.
    let terminal = std::io::stdout().is_terminal();
    let mut last_counts = None;
    let mut events = None;

    loop {
        tokio::select! {
            question = next_state(api, id, &mut events) => {
                let question = question?;

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
            }
            line = next_line(stdin) => {
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

/// The next state of the question, from its event stream, which is connected
/// first if `events` holds none.
///
/// A stream that ends, breaks off or stays silent for too long, for instance
/// because a proxy dropped or held it back, is connected again. That also
/// brings the current state, so nothing is missed.
///
/// This is a `select!` branch, so it may be dropped at any `await`: whatever
/// has to outlive it is in `events`. In particular, waiting for the stream
/// never keeps the teacher's Enter from being read.
async fn next_state(api: &Api, id: &str, events: &mut Option<Events>) -> Result<QuestionInfo> {
    loop {
        let stream = match events {
            Some(stream) => stream,
            None => match tokio::time::timeout(STREAM_SILENCE, api.question_events(id)).await {
                Ok(stream) => events.insert(stream?),
                // Not even the headers of the response came.
                Err(_) => continue,
            },
        };
        let event = match tokio::time::timeout(STREAM_SILENCE, stream.next()).await {
            Ok(Ok(Some(event))) => event,
            Ok(Ok(None) | Err(_)) | Err(_) => {
                *events = None;
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        if event.name == "gone" {
            bail!("the question is no longer on the server");
        }
        return serde_json::from_str(&event.data)
            .with_context(|| format!("the server sent an unexpected event: {}", event.data));
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
        let file = dir.join("questions.md");
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
