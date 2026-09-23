//! End-to-end test of the whole quiz: a teacher asks a question over the API,
//! students log in with OIDC and answer it in the browser, the teacher closes
//! it and collects the results.
//!
//! The identity provider is the mock in [`support`], so no credentials and no
//! network access are needed.

use std::time::Duration;

use quiz_common::api::{CreateQuestion, QuestionInfo, Results, WhoAmI};
use quiz_testsupport::{CLIENT_ID, MockIdp, User, start_server};
use reqwest::StatusCode;

const QUESTION: &str = quiz_testsupport::EXAMPLE_QUESTION;

#[tokio::test]
async fn a_question_is_asked_answered_and_closed() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &["sojkam1"]).await;

    let teacher = User::new("sojkam1", "Michal Sojka");
    let teacher_token = idp.id_token(&teacher, CLIENT_ID);
    let api = reqwest::Client::new();

    // The CLI checks who it is logged in as.
    let me: WhoAmI = api
        .get(format!("{base}/api/v1/whoami"))
        .bearer_auth(&teacher_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me.login, "sojkam1");
    assert_eq!(me.name, "Michal Sojka");
    assert!(me.is_teacher);

    // Requirement 7: the question is read from a Markdown file.
    let response = api
        .post(format!("{base}/api/v1/questions"))
        .bearer_auth(&teacher_token)
        .json(&CreateQuestion {
            source_name: "question.md".to_owned(),
            markdown: QUESTION.to_owned(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let question: QuestionInfo = response.json().await.unwrap();

    // Requirements 8 and 9: three answers, a code and an answer URL.
    assert_eq!(question.answers.len(), 3);
    assert_eq!(
        question
            .answers
            .iter()
            .map(|a| a.label.as_str())
            .collect::<Vec<_>>(),
        ["A", "B", "C"]
    );
    assert_eq!(question.code.len(), 6);
    assert!(question.code.chars().all(char::is_numeric));
    assert_eq!(question.answer_url, format!("{base}/q/{}", question.id));
    assert!(question.display_url.starts_with(&format!("{base}/d/")));
    assert!(question.open);
    assert_eq!(question.answer_count, 0);

    // Requirement 9: the presentation view shows the QR code and the code.
    let display = reqwest::Client::new()
        .get(&question.display_url)
        .send()
        .await
        .unwrap();
    assert_eq!(display.status(), StatusCode::OK);
    let body = display.text().await.unwrap();
    assert!(body.contains("</svg>"), "the QR code is missing");
    assert!(body.contains(&question.code));
    assert!(body.contains("Consider the following BASH code"));
    assert!(body.contains("<code class=\"language-bash\">"));
    dump_page("display-open.html", &body);

    // Requirements 6, 10 and 11: students log in, type the code and answer.
    let students = [
        (User::new("novakja1", "Jan Novák"), 0),
        (User::new("svobodpe", "Petra Svobodová"), 2),
        (User::new("kovarto2", "Tomáš Kovář"), 2),
    ];
    for (student, answer) in &students {
        answer_as(&base, &idp, student, &question, *answer).await;
    }

    let state: QuestionInfo = api
        .get(format!("{base}/api/v1/questions/{}", question.id))
        .bearer_auth(&teacher_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(state.answer_count, 3);
    assert_eq!(state.counts, vec![1, 0, 2]);

    // Requirement 11: the live update carries the number of answers, but not
    // the distribution while the question is open.
    let events = live_state(&format!("{}/events", question.display_url)).await;
    assert!(events.contains("event: state"), "got: {events}");
    assert!(events.contains("\"answer_count\":3"), "got: {events}");
    assert!(!events.contains("\"counts\""), "got: {events}");

    // A student may correct their answer while the question is open.
    answer_as(&base, &idp, &students[0].0, &question, 1).await;
    let state: QuestionInfo = api
        .get(format!("{base}/api/v1/questions/{}", question.id))
        .bearer_auth(&teacher_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(state.answer_count, 3, "the answer was replaced, not added");
    assert_eq!(state.counts, vec![0, 1, 2]);

    // Requirement 12: the teacher closes the question.
    let closed: QuestionInfo = api
        .post(format!("{base}/api/v1/questions/{}/close", question.id))
        .bearer_auth(&teacher_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!closed.open);
    assert!(closed.closed_at.is_some());
    assert_eq!(closed.source_name, "question.md");

    // Requirement 12: and gets the logins together with their answers.
    let results: Results = api
        .get(format!("{base}/api/v1/questions/{}/results", question.id))
        .bearer_auth(&teacher_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(results.rows.len(), 3);
    let rows: Vec<_> = results
        .rows
        .iter()
        .map(|row| (row.login.as_str(), row.answer.as_str()))
        .collect();
    // Rows are sorted by login.
    assert_eq!(
        rows,
        [("kovarto2", "C"), ("novakja1", "B"), ("svobodpe", "C")]
    );
    assert_eq!(results.rows[0].name, "Tomáš Kovář");
    // The stored text is the third answer of the Markdown file.
    let parsed = quiz_common::parse_question(QUESTION).unwrap();
    assert_eq!(results.rows[0].answer_text, parsed.answers[2].plain);

    // Requirement 12: the closed question shows a bar graph, the counts are
    // now public and the QR code is gone.
    let body = reqwest::Client::new()
        .get(&question.display_url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("class=\"bars\""), "no bar graph");
    assert!(body.contains("Otázka uzavřena"));
    assert!(!body.contains("</svg>"), "the QR code is still shown");

    let events = live_state(&format!("{}/events", question.display_url)).await;
    assert!(events.contains("\"open\":false"), "got: {events}");
    assert!(events.contains("\"counts\":[0,1,2]"), "got: {events}");

    // Answering a closed question is refused.
    let student = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    idp.login_as(&students[1].0);
    student.get(format!("{base}/")).send().await.unwrap();
    let response = student
        .post(format!("{base}/q/{}/answer", question.id))
        .form(&[("answer", "0")])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);

    dump_page("display-closed.html", &body);

    // Requirement 5: all of this ended up in the log file.
    if std::env::var_os("RUST_LOG").is_none() {
        let log = read_log().await;
        for expected in [
            "question created",
            "answer recorded",
            "question closed",
            "results exported",
            "user logged in",
            "student entered a code",
        ] {
            assert!(log.contains(expected), "{expected:?} is not in the log");
        }
        assert!(log.contains(&question.code));
        assert!(log.contains("novakja1"));
    }
}

#[tokio::test]
async fn only_teachers_with_a_valid_token_may_ask_questions() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &["sojkam1"]).await;
    let api = reqwest::Client::new();
    let url = format!("{base}/api/v1/questions");

    let ask = |token: Option<String>| {
        let request = api.post(&url).json(&CreateQuestion {
            source_name: "question.md".to_owned(),
            markdown: QUESTION.to_owned(),
        });
        let request = match token {
            Some(token) => request.bearer_auth(token),
            None => request,
        };
        async move { request.send().await.unwrap().status() }
    };

    let teacher = User::new("sojkam1", "Michal Sojka");
    let student = User::new("novakja1", "Jan Novák");

    assert_eq!(ask(None).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        ask(Some("not-a-token".to_owned())).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        ask(Some(idp.expired_id_token(&teacher, CLIENT_ID))).await,
        StatusCode::UNAUTHORIZED
    );
    // A token for a different client must not be accepted either.
    assert_eq!(
        ask(Some(idp.id_token(&teacher, "some-other-client"))).await,
        StatusCode::UNAUTHORIZED
    );
    // A valid login that is not on the teacher list.
    assert_eq!(
        ask(Some(idp.id_token(&student, CLIENT_ID))).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ask(Some(idp.id_token(&teacher, CLIENT_ID))).await,
        StatusCode::CREATED
    );

    // A teacher cannot touch somebody else's question.
    let other = start_server(&idp, &["sojkam1", "vlasakji"]).await;
    let question: QuestionInfo = api
        .post(format!("{other}/api/v1/questions"))
        .bearer_auth(idp.id_token(&teacher, CLIENT_ID))
        .json(&CreateQuestion {
            source_name: "question.md".to_owned(),
            markdown: QUESTION.to_owned(),
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let status = api
        .post(format!("{other}/api/v1/questions/{}/close", question.id))
        .bearer_auth(idp.id_token(&User::new("vlasakji", "Jiří Vlasák"), CLIENT_ID))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn html_in_a_question_reaches_the_browser_escaped() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &["sojkam1"]).await;
    let teacher = User::new("sojkam1", "Michal Sojka");

    let response = reqwest::Client::new()
        .post(format!("{base}/api/v1/questions"))
        .bearer_auth(idp.id_token(&teacher, CLIENT_ID))
        .json(&CreateQuestion {
            source_name: "xss.md".to_owned(),
            markdown: "<script>alert(1)</script>\n\nQ?\n\n---\n\n\
                       - <img src=x onerror=alert(2)>\n- plain\n"
                .to_owned(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let question: QuestionInfo = response.json().await.unwrap();

    // The projector page and the student page both show the question.
    idp.login_as(&User::new("novakja1", "Jan Novák"));
    let student = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();

    for url in [&question.display_url, &question.answer_url] {
        let page = student.get(url).send().await.unwrap().text().await.unwrap();
        assert!(
            !page.contains("<script>alert(1)"),
            "{url} rendered a script"
        );
        assert!(!page.contains("<img src=x"), "{url} rendered an image");
        assert!(page.contains("&lt;script&gt;alert(1)"), "{url}: {page}");
    }
}

#[tokio::test]
async fn an_unusable_question_is_refused_with_an_explanation() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &[]).await;
    let teacher = User::new("sojkam1", "Michal Sojka");

    let response = reqwest::Client::new()
        .post(format!("{base}/api/v1/questions"))
        .bearer_auth(idp.id_token(&teacher, CLIENT_ID))
        .json(&CreateQuestion {
            source_name: "broken.md".to_owned(),
            markdown: "A question without any answers.\n".to_owned(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: quiz_common::api::ApiError = response.json().await.unwrap();
    assert!(error.error.contains("`---`"), "got: {}", error.error);
}

#[tokio::test]
async fn a_wrong_code_is_reported_on_the_main_page() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &[]).await;
    idp.login_as(&User::new("novakja1", "Jan Novák"));

    let student = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();

    // Requirement 6: the main page is only reachable after a login, which the
    // mock provider grants right away.
    let body = student
        .get(format!("{base}/"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("Kód otázky"));
    assert!(body.contains("Jan Novák"));

    let body = student
        .post(format!("{base}/join"))
        .form(&[("code", "000000")])
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("nepatří k žádné otázce"), "got: {body}");

    // An unknown question is a plain 404 page.
    let response = student
        .get(format!("{base}/q/does-not-exist"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Logs a student in and answers the question, exactly as a browser would.
async fn answer_as(
    base: &str,
    idp: &MockIdp,
    student: &User,
    question: &QuestionInfo,
    answer: usize,
) {
    let browser = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    idp.login_as(student);

    // Requirement 10: the main page asks for the code and then redirects to
    // the very URL the QR code points to.
    let response = browser.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.text().await.unwrap().contains(&student.name));

    let response = browser
        .post(format!("{base}/join"))
        .form(&[("code", question.code.as_str())])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.url().as_str(), question.answer_url);
    let body = response.text().await.unwrap();
    assert!(body.contains("What&#x27;s wrong with it?") || body.contains("What's wrong with it?"));

    let response = browser
        .post(format!("{base}/q/{}/answer", question.id))
        .form(&[("answer", &answer.to_string())])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(
        body.contains("byla zaznamenána"),
        "the answer was not saved"
    );
    dump_page("answer-open.html", &body);
}

/// Saves a rendered page when `QUIZ_DUMP_PAGES=<directory>` is set, so that the
/// styling can be looked at in a browser without starting anything.
///
/// The stylesheet is inlined, because the saved file is opened over `file://`
/// where the server is not there to serve it.
fn dump_page(name: &str, html: &str) {
    let Some(dir) = std::env::var_os("QUIZ_DUMP_PAGES") else {
        return;
    };
    let css = concat!("<style>", include_str!("../static/app.css"), "</style>");
    let html = regex_free_replace(html, "<link rel=\"stylesheet\"", "\">", css);

    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), html).unwrap();
}

/// Replaces everything from `from` up to and including `to` with `with`.
fn regex_free_replace(text: &str, from: &str, to: &str, with: &str) -> String {
    let Some(start) = text.find(from) else {
        return text.to_owned();
    };
    let Some(end) = text[start..]
        .find(to)
        .map(|offset| start + offset + to.len())
    else {
        return text.to_owned();
    };
    format!("{}{with}{}", &text[..start], &text[end..])
}

/// Reads the first server-sent event from `url`.
async fn live_state(url: &str) -> String {
    let mut response = reqwest::Client::new().get(url).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let chunk = tokio::time::timeout(Duration::from_secs(5), response.chunk())
        .await
        .expect("the event stream sends the state right away")
        .unwrap()
        .expect("the event stream is not empty");
    String::from_utf8_lossy(&chunk).into_owned()
}

/// Waits for the asynchronous log writer to catch up and returns the log.
async fn read_log() -> String {
    let path = quiz_testsupport::log_file();
    for _ in 0..50 {
        let log = std::fs::read_to_string(&path).unwrap_or_default();
        if log.contains("results exported") {
            return log;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    std::fs::read_to_string(&path).unwrap_or_default()
}
