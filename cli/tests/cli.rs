//! Drives the compiled `quiz` binary against a real server and the mock OIDC
//! provider, including the loopback login and the CSV export.

use std::{path::PathBuf, process::Stdio, time::Duration};

use quiz_testsupport::{EXAMPLE_QUESTION, EXECUTABLE_QUESTION, MockIdp, User, start_server};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

/// The whole flow is fast; the limit is only there to fail instead of hang.
const TIMEOUT: Duration = Duration::from_secs(60);

const QUIZ: &str = env!("CARGO_BIN_EXE_quiz");

#[tokio::test]
async fn ask_publishes_a_question_collects_answers_and_writes_a_csv() {
    tokio::time::timeout(TIMEOUT, ask_flow())
        .await
        .expect("the CLI finished in time");
}

async fn ask_flow() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &["sojkam1"]).await;

    // A directory of our own: the question, the CSV and, thanks to
    // XDG_CONFIG_HOME, the token cache all live here instead of in $HOME.
    let dir = scratch("ask");
    let question = dir.join("otazky.md");
    std::fs::write(&question, EXAMPLE_QUESTION).unwrap();

    idp.login_as(&User::new("sojkam1", "Michal Sojka"));

    let mut child = Command::new(QUIZ)
        .args([
            "--server",
            &base,
            "--no-browser",
            "--login-port",
            "0",
            "ask",
        ])
        .arg(&question)
        .env("XDG_CONFIG_HOME", &dir)
        .env_remove("QUIZ_SERVER")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .expect("the quiz binary is built");

    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut printed = String::new();
    let mut code = None;
    let mut answer_url = None;

    while let Some(line) = lines.next_line().await.unwrap() {
        printed.push_str(&line);
        printed.push('\n');

        // Requirement 7: without a browser the CLI prints the URLs instead.
        if line.contains(&idp.issuer) && line.contains("/authorize") {
            // Play the part of the browser: visiting the URL logs the teacher
            // in and hands the code to the CLI's loopback listener.
            let response = reqwest::Client::new()
                .get(line.trim())
                .send()
                .await
                .expect("the login URL can be opened");
            assert!(response.status().is_success());
            assert!(response.text().await.unwrap().contains("Logged in"));
        }
        if let Some(rest) = line.strip_prefix("Question code:") {
            code = Some(rest.trim().to_owned());
        }
        if let Some(rest) = line.strip_prefix("Students answer at:") {
            answer_url = Some(rest.trim().to_owned());
        }
        if line.starts_with("Press Enter") {
            break;
        }
    }

    // The parsed question is shown before anything is published.
    let parsed = quiz_common::parse_question(EXAMPLE_QUESTION).unwrap();
    assert!(printed.contains("A) "), "{printed}");
    assert!(
        printed.contains(&format!("C) {}", parsed.answers[2].plain)),
        "{printed}"
    );

    let code = code.expect("the CLI prints the question code");
    let answer_url = answer_url.expect("the CLI prints the answer URL");
    assert_eq!(code.len(), 6);

    // A student answers, exactly as in the browser.
    let student = User::new("novakja1", "Jan Novák");
    idp.login_as(&student);
    let browser = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    browser.get(format!("{base}/")).send().await.unwrap();
    let response = browser
        .post(format!("{base}/join"))
        .form(&[("code", &code)])
        .send()
        .await
        .unwrap();
    assert_eq!(response.url().as_str(), answer_url);
    let id = response.url().path().rsplit('/').next().unwrap().to_owned();
    let response = browser
        .post(format!("{base}/q/{id}/answer"))
        .form(&[("answer", "0")])
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());

    // Requirement 12: Enter closes the question and saves the answers.
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"\n").await.unwrap();
    drop(stdin);

    while let Some(line) = lines.next_line().await.unwrap() {
        printed.push_str(&line);
        printed.push('\n');
    }
    let status = child.wait().await.unwrap();
    assert!(status.success(), "the CLI failed:\n{printed}");
    assert!(printed.contains("Answers: 1"), "{printed}");
    assert!(printed.contains("Answers saved to"), "{printed}");

    // The CSV is named after the question file plus date and time.
    let csv_files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "csv"))
        .collect();
    assert_eq!(csv_files.len(), 1, "expected exactly one CSV in {dir:?}");
    let name = csv_files[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(name.starts_with("otazky_"), "{name}");
    assert_eq!(name.len(), "otazky_2026-09-21_22-05-03.csv".len(), "{name}");

    let csv = std::fs::read_to_string(&csv_files[0]).unwrap();
    let mut rows = csv.lines();
    assert_eq!(
        rows.next().unwrap(),
        "login,name,answer,answer_text,answered_at"
    );
    let row = rows.next().expect("one answer was collected");
    assert!(row.starts_with("novakja1,Jan Novák,A,"), "{row}");
    assert!(row.contains(&parsed.answers[0].plain), "{row}");
    assert!(rows.next().is_none());

    // The token was cached, so the next command needs no browser at all.
    let cache = dir.join("quiz").join("tokens.json");
    assert!(cache.is_file(), "the token was not cached");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&cache).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the cached token is not private");
    }

    let output = Command::new(QUIZ)
        .args(["--server", &base, "--no-browser", "whoami"])
        .env("XDG_CONFIG_HOME", &dir)
        .output()
        .await
        .unwrap();
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{printed}");
    assert!(printed.contains("Michal Sojka (sojkam1)"), "{printed}");
    assert!(printed.contains("May ask questions."), "{printed}");

    // And it can be forgotten again.
    let output = Command::new(QUIZ)
        .args(["--server", &base, "logout"])
        .env("XDG_CONFIG_HOME", &dir)
        .output()
        .await
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Token forgotten"));
}

#[tokio::test]
async fn parse_checks_a_question_file_without_a_server() {
    let dir = scratch("parse");

    let good = dir.join("question.md");
    std::fs::write(&good, EXAMPLE_QUESTION).unwrap();
    let output = Command::new(QUIZ)
        .arg("parse")
        .arg(&good)
        .output()
        .await
        .unwrap();
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{printed}");
    assert!(
        printed.contains("valid question with 3 possible answers"),
        "{printed}"
    );

    let bad = dir.join("broken.md");
    std::fs::write(&bad, "Where are the answers?\n").unwrap();
    let output = Command::new(QUIZ)
        .arg("parse")
        .arg(&bad)
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    let printed = String::from_utf8_lossy(&output.stderr);
    assert!(
        printed.contains("broken.md is not a usable question"),
        "{printed}"
    );
    assert!(printed.contains("no `---` separator found"), "{printed}");
}

#[cfg(unix)]
#[tokio::test]
async fn an_executable_question_loses_its_shebang_line() {
    use std::os::unix::fs::PermissionsExt;

    let dir = scratch("shebang");
    let question = dir.join("otazky.md");
    std::fs::write(&question, EXECUTABLE_QUESTION).unwrap();
    let shebang = EXECUTABLE_QUESTION.lines().next().unwrap();
    assert!(shebang.starts_with("#!"), "the fixture has no shebang line");

    // As a plain Markdown file the shebang is text like any other.
    let output = Command::new(QUIZ)
        .arg("parse")
        .arg(&question)
        .output()
        .await
        .unwrap();
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{printed}");
    assert!(printed.contains(shebang), "{printed}");

    // Made executable, the file is a script and the shebang belongs to it.
    std::fs::set_permissions(&question, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(QUIZ)
        .arg("parse")
        .arg(&question)
        .output()
        .await
        .unwrap();
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{printed}");
    assert!(!printed.contains("#!"), "{printed}");
    assert!(
        printed.starts_with("Consider the following BASH code:"),
        "{printed}"
    );
    assert!(
        printed.contains("valid question with 3 possible answers"),
        "{printed}"
    );
}

/// A scratch directory of this test process.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("quiz-cli-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
