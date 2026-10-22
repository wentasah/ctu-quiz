//! End-to-end test of the whole quiz: a teacher asks a question over the API,
//! students log in with OIDC and answer it in the browser, the teacher closes
//! it and collects the results.
//!
//! The identity provider is the mock in [`support`], so no credentials and no
//! network access are needed.

use std::time::Duration;

use quiz_common::api::{
    CreateQuestion, QuestionInfo, Results, RoomClosed, RoomStatus, StudentView, WhoAmI,
};
use quiz_testsupport::{
    CLIENT_ID, MockIdp, User, question_on_page, start_server, start_server_with,
};
use reqwest::StatusCode;

const QUESTION: &str = quiz_testsupport::EXAMPLE_QUESTION;

#[tokio::test]
async fn a_question_is_asked_answered_and_closed() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &["sojkam1"]).await;

    let teacher = User::new("sojkam1", "Michal Sojka");
    let teacher_token = idp.access_token(&teacher, CLIENT_ID);
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
            view: StudentView::default(),
            shuffle: false,
            close_after: None,
            start_countdown: false,
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
    assert!(
        question.answer_url.starts_with(&format!("{base}/r/")),
        "students are sent to the teacher's room: {}",
        question.answer_url
    );
    assert_eq!(question.display_url, format!("{base}/d"));
    assert!(question.open);
    assert_eq!(question.answer_count, 0);

    // Requirement 9: the presentation view shows the QR code and the code.
    let projector = browser_as(&base, &idp, &teacher).await;
    let display = projector.get(&question.display_url).send().await.unwrap();
    assert_eq!(display.status(), StatusCode::OK);
    let body = display.text().await.unwrap();
    assert!(body.contains("</svg>"), "the QR code is missing");
    assert!(body.contains(&question.code));
    assert!(body.contains("Consider the following BASH code"));
    assert!(body.contains("<code class=\"language-bash\">"));
    assert!(body.contains("<title>Kvíz – otázka 1</title>"), "{body}");
    assert!(!body.contains("question.md"), "the file name is shown");
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
    assert_eq!(state.people, 3, "those who answered were in the room");
    assert_eq!(state.counts, vec![1, 0, 2]);

    // Requirement 11: the live update carries the number of answers, but not
    // the distribution while the question is open.
    let events = live_state_as(&projector, &format!("{}/events", question.display_url)).await;
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
    let body = projector
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

    let events = live_state_as(&projector, &format!("{}/events", question.display_url)).await;
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
        .post(format!("{}/answer", question.answer_url))
        .form(&[("question", question.id.as_str()), ("answer", "0")])
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
            view: StudentView::default(),
            shuffle: false,
            close_after: None,
            start_countdown: false,
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
        ask(Some(idp.expired_access_token(&teacher, CLIENT_ID))).await,
        StatusCode::UNAUTHORIZED
    );
    // A token for a different API must not be accepted either.
    assert_eq!(
        ask(Some(idp.access_token(&teacher, "some-other-client"))).await,
        StatusCode::UNAUTHORIZED
    );
    // A student's token with the claims of the teacher: the signature no
    // longer matches.
    let forged = {
        let student = idp.access_token(&student, CLIENT_ID);
        let teacher = idp.access_token(&teacher, CLIENT_ID);
        let student: Vec<&str> = student.split('.').collect();
        let teacher: Vec<&str> = teacher.split('.').collect();
        format!("{}.{}.{}", student[0], teacher[1], student[2])
    };
    assert_eq!(ask(Some(forged)).await, StatusCode::UNAUTHORIZED);
    // Nor an ID token, which only tells its client who logged in, even when
    // that client is the server itself.
    assert_eq!(
        ask(Some(idp.id_token(&teacher, CLIENT_ID))).await,
        StatusCode::UNAUTHORIZED
    );
    // A valid login that is not on the teacher list.
    assert_eq!(
        ask(Some(idp.access_token(&student, CLIENT_ID))).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ask(Some(idp.access_token(&teacher, CLIENT_ID))).await,
        StatusCode::CREATED
    );

    // A teacher cannot touch somebody else's question.
    let other = start_server(&idp, &["sojkam1", "vlasakji"]).await;
    let question: QuestionInfo = api
        .post(format!("{other}/api/v1/questions"))
        .bearer_auth(idp.access_token(&teacher, CLIENT_ID))
        .json(&CreateQuestion {
            source_name: "question.md".to_owned(),
            markdown: QUESTION.to_owned(),
            view: StudentView::default(),
            shuffle: false,
            close_after: None,
            start_countdown: false,
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let status = api
        .post(format!("{other}/api/v1/questions/{}/close", question.id))
        .bearer_auth(idp.access_token(&User::new("vlasakji", "Jiří Vlasák"), CLIENT_ID))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_teacher_role_in_the_access_token_allows_asking_questions() {
    let idp = MockIdp::start().await;
    let base = start_server_with(&idp, &["vlasakji"], &["--teacher-roles", "teacher"]).await;
    let api = reqwest::Client::new();

    let ask = |user: User| {
        let request = api
            .post(format!("{base}/api/v1/questions"))
            .bearer_auth(idp.access_token(&user, CLIENT_ID))
            .json(&CreateQuestion {
                source_name: "question.md".to_owned(),
                markdown: QUESTION.to_owned(),
                view: StudentView::default(),
                shuffle: false,
                close_after: None,
                start_countdown: false,
            });
        async move { request.send().await.unwrap().status() }
    };
    let whoami = |user: User| {
        let request = api
            .get(format!("{base}/api/v1/whoami"))
            .bearer_auth(idp.access_token(&user, CLIENT_ID));
        async move {
            request
                .send()
                .await
                .unwrap()
                .json::<WhoAmI>()
                .await
                .unwrap()
        }
    };

    let employee = User::new("sojkam1", "Michal Sojka").with_roles(&["employee", "teacher"]);
    let student = User::new("novakja1", "Jan Novák").with_roles(&["student"]);
    let listed = User::new("vlasakji", "Jiří Vlasák");

    assert!(whoami(employee.clone()).await.is_teacher);
    assert!(!whoami(student.clone()).await.is_teacher);
    assert_eq!(ask(employee).await, StatusCode::CREATED);
    assert_eq!(ask(student).await, StatusCode::FORBIDDEN);
    // The list of logins still works next to the roles.
    assert_eq!(ask(listed).await, StatusCode::CREATED);
}

#[tokio::test]
async fn html_in_a_question_reaches_the_browser_escaped() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &["sojkam1"]).await;
    let teacher = User::new("sojkam1", "Michal Sojka");

    let response = reqwest::Client::new()
        .post(format!("{base}/api/v1/questions"))
        .bearer_auth(idp.access_token(&teacher, CLIENT_ID))
        .json(&CreateQuestion {
            source_name: "xss.md".to_owned(),
            markdown: "<script>alert(1)</script>\n\nQ?\n\n---\n\n\
                       - <img src=x onerror=alert(2)>\n- plain\n"
                .to_owned(),
            view: StudentView::default(),
            shuffle: false,
            close_after: None,
            start_countdown: false,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let question: QuestionInfo = response.json().await.unwrap();

    // The projector page and the student page both show the question.
    let projector = browser_as(&base, &idp, &teacher).await;
    let student = browser_as(&base, &idp, &User::new("novakja1", "Jan Novák")).await;

    for (browser, url) in [
        (&projector, &question.display_url),
        (&student, &question.answer_url),
    ] {
        let page = browser.get(url).send().await.unwrap().text().await.unwrap();
        assert!(
            !page.contains("<script>alert(1)"),
            "{url} rendered a script"
        );
        assert!(!page.contains("<img src=x"), "{url} rendered an image");
        assert!(page.contains("&lt;script&gt;alert(1)"), "{url}: {page}");
    }
}

#[tokio::test]
async fn the_next_question_reaches_a_page_that_is_already_open() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &["sojkam1"]).await;
    let teacher = User::new("sojkam1", "Michal Sojka");
    let token = idp.access_token(&teacher, CLIENT_ID);
    let api = reqwest::Client::new();

    let ask = |markdown: String| {
        let request = api
            .post(format!("{base}/api/v1/questions"))
            .bearer_auth(token.clone())
            .json(&CreateQuestion {
                source_name: "otazky.md".to_owned(),
                markdown,
                view: StudentView::default(),
                shuffle: false,
                close_after: None,
                start_countdown: false,
            });
        async move {
            let response = request.send().await.unwrap();
            assert_eq!(response.status(), StatusCode::CREATED);
            response.json::<QuestionInfo>().await.unwrap()
        }
    };

    let first = ask("First?\n\n---\n\n- one\n- two\n".to_owned()).await;

    // A student scans the QR code once.
    idp.login_as(&User::new("novakja1", "Jan Novák"));
    let student = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    student.get(format!("{base}/")).send().await.unwrap();
    let response = student
        .post(format!("{base}/join"))
        .form(&[("code", first.code.as_str())])
        .send()
        .await
        .unwrap();
    assert_eq!(response.url().as_str(), first.answer_url);
    assert!(response.text().await.unwrap().contains("First?"));

    // The teacher asks the next one. It is the same room, so the code and
    // the URL students use do not change.
    let second = ask("Second?\n\n---\n\n- one\n- two\n".to_owned()).await;
    assert_ne!(second.id, first.id);
    assert_eq!(second.code, first.code);
    assert_eq!(second.answer_url, first.answer_url);
    assert_eq!(second.display_url, first.display_url);

    // The page the student has open is told to render itself again, and it
    // then shows the new question - without any code being typed again.
    let events = live_state_as(&student, &format!("{}/events", first.answer_url)).await;
    assert!(
        events.contains(&format!("\"question\":\"{}\"", second.id)),
        "the room announces the new question: {events}"
    );

    let body = student
        .get(&first.answer_url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("Second?"), "{body}");
    assert!(!body.contains("First?"), "{body}");
    assert!(body.contains("<title>Kvíz – otázka 2</title>"), "{body}");
    assert_eq!(question_on_page(&body).as_deref(), Some(second.id.as_str()));

    // An answer sent for the question that was on the page a moment ago is
    // not recorded; the student is shown what is being asked now.
    let response = student
        .post(format!("{}/answer", first.answer_url))
        .form(&[("question", first.id.as_str()), ("answer", "0")])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.url().as_str(), first.answer_url);
    let state: QuestionInfo = api
        .get(format!("{base}/api/v1/questions/{}", first.id))
        .bearer_auth(token.clone())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(state.answer_count, 0, "the stale answer was not recorded");

    // Answering the question that is being asked works as usual.
    let response = student
        .post(format!("{}/answer", first.answer_url))
        .form(&[("question", second.id.as_str()), ("answer", "1")])
        .send()
        .await
        .unwrap();
    assert!(response.text().await.unwrap().contains("byla zaznamenána"));

    // Once the teacher closes it, the page waits for the one after that.
    api.post(format!("{base}/api/v1/questions/{}/close", second.id))
        .bearer_auth(token.clone())
        .send()
        .await
        .unwrap();
    let body = student
        .get(&first.answer_url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("Otázka je uzavřena"), "{body}");
    assert!(body.contains("Další otázka se zde objeví sama"), "{body}");
}

#[tokio::test]
async fn one_presentation_window_serves_the_whole_lecture() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &["sojkam1"]).await;
    let teacher = User::new("sojkam1", "Michal Sojka");
    let token = idp.access_token(&teacher, CLIENT_ID);
    let api = reqwest::Client::new();

    let ask = |markdown: &str| {
        let request = api
            .post(format!("{base}/api/v1/questions"))
            .bearer_auth(token.clone())
            .json(&CreateQuestion {
                source_name: "otazky.md".to_owned(),
                markdown: markdown.to_owned(),
                view: StudentView::default(),
                shuffle: false,
                close_after: None,
                start_countdown: false,
            });
        async move {
            let response = request.send().await.unwrap();
            assert_eq!(response.status(), StatusCode::CREATED);
            response.json::<QuestionInfo>().await.unwrap()
        }
    };

    // Nothing shows the first question yet, so the CLI opens a window.
    let first = ask("First?\n\n---\n\n- one\n- two\n").await;
    assert!(!first.presented);

    // The window follows the room.
    let projector = browser_as(&base, &idp, &teacher).await;
    let mut window = projector
        .get(format!("{}/events", first.display_url))
        .send()
        .await
        .unwrap();
    let next_event = async |window: &mut reqwest::Response| {
        let chunk = tokio::time::timeout(Duration::from_secs(5), window.chunk())
            .await
            .expect("the event stream sends the state")
            .unwrap()
            .expect("the event stream goes on");
        String::from_utf8_lossy(&chunk).into_owned()
    };
    assert!(next_event(&mut window).await.contains(&first.id));

    // The next question goes to the window that is already open, which is
    // told to render itself again.
    let second = ask("Second?\n\n---\n\n- one\n- two\n").await;
    assert!(second.presented, "the open window was not noticed");
    assert_eq!(second.display_url, first.display_url);
    assert!(next_event(&mut window).await.contains(&second.id));
    let page = projector
        .get(&first.display_url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("Second?"), "{page}");
    assert!(!page.contains("First?"), "{page}");
    assert!(page.contains("<title>Kvíz – otázka 2</title>"), "{page}");

    // Once the window is closed, the CLI has to open another one.
    drop(window);
    for attempt in 0.. {
        if !ask("Third?\n\n---\n\n- one\n- two\n").await.presented {
            break;
        }
        assert!(attempt < 50, "the closed window is still counted");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn the_question_can_be_closed_in_the_presentation_view() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &["sojkam1"]).await;
    let teacher = User::new("sojkam1", "Michal Sojka");
    let token = idp.access_token(&teacher, CLIENT_ID);
    let api = reqwest::Client::new();
    let question: QuestionInfo = api
        .post(format!("{base}/api/v1/questions"))
        .bearer_auth(&token)
        .json(&CreateQuestion {
            source_name: "otazky.md".to_owned(),
            markdown: QUESTION.to_owned(),
            view: StudentView::default(),
            shuffle: false,
            close_after: None,
            start_countdown: false,
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let is_open = async || {
        api.get(format!("{base}/api/v1/questions/{}", question.id))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json::<QuestionInfo>()
            .await
            .unwrap()
            .open
    };
    let projector = browser_as(&base, &idp, &teacher).await;
    let close = |browser: &reqwest::Client, id: &str| {
        browser
            .post(format!("{}/close", question.display_url))
            .form(&[("question", id)])
            .send()
    };

    let page = projector
        .get(&question.display_url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("Uzavřít otázku"), "{page}");
    assert!(!page.contains("countdown"), "nothing closes it by itself");

    // A question that is not on the screen stays open, and so does the one
    // that is for anybody but the teacher who asked it.
    let response = close(&projector, "other").await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let student = browser_as(&base, &idp, &User::new("novakja1", "Jan Novák")).await;
    let response = close(&student, &question.id).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(is_open().await);

    // The button closes it and shows the results.
    let response = close(&projector, &question.id).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let page = response.text().await.unwrap();
    assert!(page.contains("Otázka uzavřena"), "{page}");
    assert!(!page.contains("Uzavřít otázku"), "{page}");
    assert!(!is_open().await);
}

#[tokio::test]
async fn close_after_counts_down_from_when_the_projector_shows_the_question() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &["sojkam1"]).await;
    let teacher = User::new("sojkam1", "Michal Sojka");
    let token = idp.access_token(&teacher, CLIENT_ID);
    let api = reqwest::Client::new();
    let ask = |close_after: u64, start_countdown: bool| {
        let request = api
            .post(format!("{base}/api/v1/questions"))
            .bearer_auth(&token)
            .json(&CreateQuestion {
                source_name: "otazky.md".to_owned(),
                markdown: QUESTION.to_owned(),
                view: StudentView::default(),
                shuffle: false,
                close_after: Some(close_after),
                start_countdown,
            });
        async move {
            request
                .send()
                .await
                .unwrap()
                .json::<QuestionInfo>()
                .await
                .unwrap()
        }
    };
    let is_open = async |question: &QuestionInfo| {
        api.get(format!("{base}/api/v1/questions/{}", question.id))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json::<QuestionInfo>()
            .await
            .unwrap()
            .open
    };
    let closes_within = async |question: &QuestionInfo, limit: Duration| {
        let start = std::time::Instant::now();
        while is_open(question).await {
            assert!(start.elapsed() < limit, "the question is still open");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };

    // The time the teacher takes to log in at the projector does not count.
    let question = ask(1, false).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(is_open(&question).await, "it closed before it was shown");

    let page = browser_as(&base, &idp, &teacher)
        .await
        .get(&question.display_url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("class=\"countdown\""), "{page}");
    assert!(page.contains("data-total-ms=\"1000\""), "{page}");
    let remaining: i64 = page
        .split_once("data-remaining-ms=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(ms, _)| ms.parse().unwrap())
        .expect("the time left is on the button");
    assert!((500..=1000).contains(&remaining), "{remaining}");

    // Then the server closes it by itself.
    closes_within(&question, Duration::from_secs(5)).await;

    // `quiz ask --no-browser` starts the countdown right away.
    let question = ask(1, true).await;
    closes_within(&question, Duration::from_secs(5)).await;
}

#[tokio::test]
async fn a_hidden_question_stays_on_the_projector() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &["sojkam1"]).await;
    let teacher = User::new("sojkam1", "Michal Sojka");
    let token = idp.access_token(&teacher, CLIENT_ID);
    let api = reqwest::Client::new();

    let markdown = "# Roury\n\nWhat is wrong with the pipeline?\n\n---\n\n\
                    - a subshell swallows the variable\n- nothing at all\n";
    let ask = |view: StudentView| {
        let request = api
            .post(format!("{base}/api/v1/questions"))
            .bearer_auth(token.clone())
            .json(&CreateQuestion {
                source_name: "otazky.md".to_owned(),
                markdown: markdown.to_owned(),
                view,
                shuffle: false,
                close_after: None,
                start_countdown: false,
            });
        async move {
            request
                .send()
                .await
                .unwrap()
                .json::<QuestionInfo>()
                .await
                .unwrap()
        }
    };

    // Only the answers on the phones: the question is for the projector.
    let question = ask(StudentView {
        show_question: false,
        show_answers: true,
    })
    .await;
    assert!(!question.view.show_question);

    idp.login_as(&User::new("novakja1", "Jan Novák"));
    let student = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    student.get(format!("{base}/")).send().await.unwrap();
    let page = student
        .get(&question.answer_url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!page.contains("wrong with the pipeline"), "{page}");
    assert!(
        !page.contains("Roury"),
        "the heading is part of the question"
    );
    assert!(page.contains("Otázka je jen na plátně"), "{page}");
    assert!(page.contains("a subshell swallows the variable"), "{page}");

    // The teacher's screen shows everything, which is the whole point.
    let display = browser_as(&base, &idp, &teacher)
        .await
        .get(&question.display_url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(display.contains("wrong with the pipeline"), "{display}");
    assert!(display.contains("a subshell swallows the variable"));

    // Nothing but the labels: the phone is a keypad with A and B on it.
    let question = ask(StudentView {
        show_question: false,
        show_answers: false,
    })
    .await;
    let page = student
        .get(&question.answer_url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!page.contains("wrong with the pipeline"), "{page}");
    assert!(!page.contains("a subshell swallows"), "{page}");
    assert!(page.contains("labels-only"), "{page}");
    assert!(page.contains(">A</span>"), "{page}");
    assert!(page.contains(">B</span>"), "{page}");
    dump_page("answer-labels-only.html", &page);

    // Answering works the same way, and so do the results.
    let response = student
        .post(format!("{}/answer", question.answer_url))
        .form(&[("question", question.id.as_str()), ("answer", "0")])
        .send()
        .await
        .unwrap();
    assert!(response.text().await.unwrap().contains("byla zaznamenána"));

    api.post(format!("{base}/api/v1/questions/{}/close", question.id))
        .bearer_auth(token.clone())
        .send()
        .await
        .unwrap();
    let page = student
        .get(&question.answer_url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("class=\"bars\""), "the bar graph is there");
    assert!(!page.contains("a subshell swallows"), "{page}");

    // The teacher's CSV keeps the real text, hidden or not.
    let results: Results = api
        .get(format!("{base}/api/v1/questions/{}/results", question.id))
        .bearer_auth(token.clone())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        results.rows[0].answer_text,
        "a subshell swallows the variable"
    );
}

#[tokio::test]
async fn shuffled_answers_are_the_same_for_everybody() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &["sojkam1"]).await;
    let teacher = User::new("sojkam1", "Michal Sojka");
    let token = idp.access_token(&teacher, CLIENT_ID);
    let api = reqwest::Client::new();

    // Eight answers, so an order that comes out unshuffled by chance is a
    // 1 in 40320 event rather than something to worry about.
    let markdown = format!(
        "Which one?\n\n---\n\n{}",
        (0..8).fold(String::new(), |mut list, i| {
            list.push_str(&format!("- answer number {i}\n"));
            list
        })
    );
    let of_the_file: Vec<String> = quiz_common::parse_question(&markdown)
        .unwrap()
        .answers
        .iter()
        .map(|answer| answer.plain.clone())
        .collect();

    let ask = |shuffle: bool| {
        let request = api
            .post(format!("{base}/api/v1/questions"))
            .bearer_auth(token.clone())
            .json(&CreateQuestion {
                source_name: "otazky.md".to_owned(),
                markdown: markdown.clone(),
                view: StudentView::default(),
                shuffle,
                close_after: None,
                start_countdown: false,
            });
        async move {
            request
                .send()
                .await
                .unwrap()
                .json::<QuestionInfo>()
                .await
                .unwrap()
        }
    };

    // Without the flag the answers are in the order of the Markdown file.
    let plain = ask(false).await;
    let order: Vec<_> = plain.answers.iter().map(|a| a.text.clone()).collect();
    assert_eq!(order, of_the_file);

    let question = ask(true).await;
    let shuffled: Vec<_> = question.answers.iter().map(|a| a.text.clone()).collect();
    assert_ne!(shuffled, of_the_file, "the answers were not shuffled");
    let mut sorted = shuffled.clone();
    sorted.sort();
    let mut expected = of_the_file.clone();
    expected.sort();
    assert_eq!(sorted, expected, "an answer was lost or duplicated");

    // The labels still run down the screen.
    let labels: Vec<_> = question.answers.iter().map(|a| a.label.as_str()).collect();
    assert_eq!(labels, ["A", "B", "C", "D", "E", "F", "G", "H"]);

    // Everybody sees that one order: the projector and the phones alike.
    let display = browser_as(&base, &idp, &teacher)
        .await
        .get(&question.display_url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    idp.login_as(&User::new("novakja1", "Jan Novák"));
    let student = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    student.get(format!("{base}/")).send().await.unwrap();
    let page = student
        .get(&question.answer_url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(order_of(&display, &of_the_file), shuffled);
    assert_eq!(order_of(&page, &of_the_file), shuffled);

    // And the answer a student picks is the one that was in that place.
    let response = student
        .post(format!("{}/answer", question.answer_url))
        .form(&[("question", question.id.as_str()), ("answer", "0")])
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    api.post(format!("{base}/api/v1/questions/{}/close", question.id))
        .bearer_auth(token.clone())
        .send()
        .await
        .unwrap();
    let results: Results = api
        .get(format!("{base}/api/v1/questions/{}/results", question.id))
        .bearer_auth(token.clone())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(results.rows[0].answer, "A");
    assert_eq!(results.rows[0].answer_text, shuffled[0]);
}

#[tokio::test]
async fn points_stay_hidden_until_the_teacher_reveals_them() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &["sojkam1"]).await;
    let teacher = User::new("sojkam1", "Michal Sojka");
    let token = idp.access_token(&teacher, CLIENT_ID);
    let api = reqwest::Client::new();

    let question: QuestionInfo = api
        .post(format!("{base}/api/v1/questions"))
        .bearer_auth(token.clone())
        .json(&CreateQuestion {
            source_name: "otazky.md".to_owned(),
            markdown: "Which one?\n\n---\n\n- [ ] the wrong one {-1}\n- [x] the right one\n"
                .to_owned(),
            view: StudentView::default(),
            shuffle: false,
            close_after: None,
            start_countdown: false,
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let points: Vec<_> = question.answers.iter().map(|a| a.points).collect();
    assert_eq!(points, [Some(-1.0), Some(1.0)]);

    idp.login_as(&User::new("novakja1", "Jan Novák"));
    let student = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    student.get(format!("{base}/")).send().await.unwrap();
    let projector = browser_as(&base, &idp, &teacher).await;
    let page = |client: reqwest::Client, url: String| async move {
        client.get(url).send().await.unwrap().text().await.unwrap()
    };

    // While the question is open, nothing tells which answer is right.
    for open in [
        page(student.clone(), question.answer_url.clone()).await,
        page(projector.clone(), question.display_url.clone()).await,
    ] {
        assert!(open.contains("the right one"), "{open}");
        for leak in ["[x]", "[ ]", "{-1}", "checkbox", "correct", "&#10003;"] {
            assert!(!open.contains(leak), "{leak} in {open}");
        }
    }

    student
        .post(format!("{}/answer", question.answer_url))
        .form(&[("question", question.id.as_str()), ("answer", "0")])
        .send()
        .await
        .unwrap();

    let reveal = || {
        api.post(format!("{base}/api/v1/questions/{}/reveal", question.id))
            .bearer_auth(token.clone())
            .send()
    };
    // An open question cannot be revealed: students could still change
    // their answer.
    let response = reveal().await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(response.text().await.unwrap().contains("still open"));

    api.post(format!("{base}/api/v1/questions/{}/close", question.id))
        .bearer_auth(token.clone())
        .send()
        .await
        .unwrap();

    // Closing shows the results, but not yet which answer is right, so
    // that the teacher can talk about them first.
    for closed in [
        page(student.clone(), question.answer_url.clone()).await,
        page(projector.clone(), question.display_url.clone()).await,
    ] {
        assert!(closed.contains("class=\"bars\""), "{closed}");
        assert!(closed.contains("data-revealed=\"false\""), "{closed}");
        assert!(!closed.contains(" correct\""), "{closed}");
    }
    let closed = page(student.clone(), question.answer_url.clone()).await;
    assert!(
        closed.contains("Vaše odpověď byla <strong>A</strong>"),
        "{closed}"
    );

    let response = reveal().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.json::<QuestionInfo>().await.unwrap().revealed);
    let events = live_state_as(&projector, &format!("{}/events", question.display_url)).await;
    assert!(events.contains("\"revealed\":true"), "got: {events}");

    // Once revealed, the results mark the right answer.
    for closed in [
        page(student.clone(), question.answer_url.clone()).await,
        page(projector.clone(), question.display_url.clone()).await,
    ] {
        assert_eq!(closed.matches(" correct\"").count(), 1, "{closed}");
        let right = closed.find(" correct\"").unwrap();
        assert!(closed[right..].contains("the right one"), "{closed}");
    }
    // The student learns that their answer was wrong.
    let closed = page(student.clone(), question.answer_url.clone()).await;
    let wrong = "<p class=\"status wrong\">Vaše odpověď <strong>A</strong> byla špatná.";
    assert!(closed.contains(wrong), "{closed}");

    let results: Results = api
        .get(format!("{base}/api/v1/questions/{}/results", question.id))
        .bearer_auth(token.clone())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(results.rows[0].points, Some(-1.0));
}

#[tokio::test]
async fn pages_link_the_stylesheet_and_script_of_this_version() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &["sojkam1"]).await;
    let teacher = User::new("sojkam1", "Michal Sojka");
    let question: QuestionInfo = reqwest::Client::new()
        .post(format!("{base}/api/v1/questions"))
        .bearer_auth(idp.access_token(&teacher, CLIENT_ID))
        .json(&CreateQuestion {
            source_name: "otazky.md".to_owned(),
            markdown: QUESTION.to_owned(),
            view: StudentView::default(),
            shuffle: false,
            close_after: None,
            start_countdown: false,
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let page = browser_as(&base, &idp, &teacher)
        .await
        .get(&question.display_url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    // A new server changes the URLs, so no browser keeps an old copy.
    for (file, expected) in [("app.css", "body.display"), ("app.js", "EventSource")] {
        let at = page.find(&format!("/static/{file}?v=")).expect(file);
        let url = &page[at..at + page[at..].find('"').unwrap()];
        let body = reqwest::get(format!("{base}{url}"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(body.contains(expected), "{url}");
    }
}

/// The answers of `texts` in the order in which they appear in a page.
fn order_of(page: &str, texts: &[String]) -> Vec<String> {
    let mut found: Vec<(usize, String)> = texts
        .iter()
        .filter_map(|text| page.find(text.as_str()).map(|at| (at, text.clone())))
        .collect();
    found.sort();
    found.into_iter().map(|(_, text)| text).collect()
}

#[tokio::test]
async fn logging_out_removes_the_room() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &["sojkam1"]).await;
    let teacher = User::new("sojkam1", "Michal Sojka");
    let token = idp.access_token(&teacher, CLIENT_ID);
    let api = reqwest::Client::new();

    let response = api
        .post(format!("{base}/api/v1/questions"))
        .bearer_auth(&token)
        .json(&CreateQuestion {
            source_name: "otazky.md".to_owned(),
            markdown: QUESTION.to_owned(),
            view: StudentView::default(),
            shuffle: false,
            close_after: None,
            start_countdown: false,
        })
        .send()
        .await
        .unwrap();
    let question: QuestionInfo = response.json().await.unwrap();

    // A student is in the room, with the page open.
    idp.login_as(&User::new("novakja1", "Jan Novák"));
    let student = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    student.get(format!("{base}/")).send().await.unwrap();
    let response = student
        .post(format!("{base}/join"))
        .form(&[("code", question.code.as_str())])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut events = student
        .get(format!("{}/events", question.answer_url))
        .send()
        .await
        .unwrap();

    // `quiz logout` closes the room before it forgets the token.
    let closed: RoomClosed = api
        .post(format!("{base}/api/v1/room/close"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(closed.closed);
    assert_eq!(closed.questions, 1);

    // The page that is still open is told to have another look.
    let mut seen = String::new();
    while !seen.contains("event: gone") {
        let chunk = tokio::time::timeout(Duration::from_secs(5), events.chunk())
            .await
            .expect("the stream says that the room is gone")
            .unwrap()
            .expect("the stream is not over before it says so");
        seen.push_str(&String::from_utf8_lossy(&chunk));
    }

    // And there is nothing left to come back to.
    let response = student.get(&question.answer_url).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = browser_as(&base, &idp, &teacher)
        .await
        .get(&question.display_url)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = student
        .post(format!("{base}/join"))
        .form(&[("code", question.code.as_str())])
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("nepatří k žádné otázce"), "got: {body}");

    // Logging out again has nothing to close.
    let closed: RoomClosed = api
        .post(format!("{base}/api/v1/room/close"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!closed.closed);
    assert_eq!(closed.questions, 0);

    // The next lecture starts from scratch.
    let next: QuestionInfo = api
        .post(format!("{base}/api/v1/questions"))
        .bearer_auth(&token)
        .json(&CreateQuestion {
            source_name: "otazky.md".to_owned(),
            markdown: QUESTION.to_owned(),
            view: StudentView::default(),
            shuffle: false,
            close_after: None,
            start_countdown: false,
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_ne!(next.answer_url, question.answer_url);
    assert_ne!(next.code, question.code);
}

#[tokio::test]
async fn the_teacher_sees_the_room_and_the_questions_kept_in_it() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &["sojkam1"]).await;
    let token = idp.access_token(&User::new("sojkam1", "Michal Sojka"), CLIENT_ID);
    let api = reqwest::Client::new();
    let status = || async {
        api.get(format!("{base}/api/v1/room"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json::<RoomStatus>()
            .await
            .unwrap()
    };
    let ask = || async {
        api.post(format!("{base}/api/v1/questions"))
            .bearer_auth(&token)
            .json(&CreateQuestion {
                source_name: "otazky.md".to_owned(),
                markdown: QUESTION.to_owned(),
                view: StudentView::default(),
                shuffle: false,
                close_after: None,
                start_countdown: false,
            })
            .send()
            .await
            .unwrap()
            .json::<QuestionInfo>()
            .await
            .unwrap()
    };

    // Nothing asked yet, so there is no room.
    assert!(status().await.room.is_none());

    let first = ask().await;
    api.post(format!("{base}/api/v1/questions/{}/close", first.id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    let second = ask().await;

    let room = status().await.room.expect("asking opened a room");
    assert_eq!(room.code, first.code);
    assert_eq!(room.answer_url, first.answer_url);
    assert_eq!(room.current.as_deref(), Some(second.id.as_str()));
    assert!(room.expires_at > room.last_used_at);

    // The earlier question is still kept, closed, before the current one.
    let kept: Vec<_> = room
        .questions
        .iter()
        .map(|q| (q.number, q.info.id.as_str(), q.info.open))
        .collect();
    assert_eq!(
        kept,
        [(1, first.id.as_str(), false), (2, second.id.as_str(), true)]
    );
    assert!(room.questions[0].expires_at > room.questions[0].info.created_at);

    // Another teacher does not see this room.
    let other = idp.access_token(&User::new("novakja1", "Jan Novák"), CLIENT_ID);
    let theirs: RoomStatus = api
        .get(format!("{base}/api/v1/room"))
        .bearer_auth(&other)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(theirs.room.is_none());

    // Once the room is closed, there is nothing to show.
    api.post(format!("{base}/api/v1/room/close"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert!(status().await.room.is_none());
}

#[tokio::test]
async fn an_unusable_question_is_refused_with_an_explanation() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &[]).await;
    let teacher = User::new("sojkam1", "Michal Sojka");

    let response = reqwest::Client::new()
        .post(format!("{base}/api/v1/questions"))
        .bearer_auth(idp.access_token(&teacher, CLIENT_ID))
        .json(&CreateQuestion {
            source_name: "broken.md".to_owned(),
            markdown: "A question without any answers.\n".to_owned(),
            view: StudentView::default(),
            shuffle: false,
            close_after: None,
            start_countdown: false,
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

    // An unknown room is a plain 404 page.
    let response = student
        .get(format!("{base}/r/does-not-exist"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_teacher_sees_how_many_students_are_in_the_room() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &["sojkam1"]).await;
    let teacher = User::new("sojkam1", "Michal Sojka");
    let token = idp.access_token(&teacher, CLIENT_ID);
    let api = reqwest::Client::new();

    let question: QuestionInfo = api
        .post(format!("{base}/api/v1/questions"))
        .bearer_auth(&token)
        .json(&CreateQuestion {
            source_name: "otazky.md".to_owned(),
            markdown: "Here?\n\n---\n\n- yes\n- no\n".to_owned(),
            view: StudentView::default(),
            shuffle: false,
            close_after: None,
            start_countdown: false,
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(question.people, 0);

    let people = || {
        let request = api
            .get(format!("{base}/api/v1/questions/{}", question.id))
            .bearer_auth(&token);
        async move {
            let info: QuestionInfo = request.send().await.unwrap().json().await.unwrap();
            (info.answer_count, info.people)
        }
    };

    // A student opens the room, in two tabs, and has not answered yet.
    idp.login_as(&User::new("novakja1", "Jan Novák"));
    let student = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    student.get(format!("{base}/")).send().await.unwrap();
    let mut tabs = Vec::new();
    for _ in 0..2 {
        let mut events = student
            .get(format!("{}/events", question.answer_url))
            .send()
            .await
            .unwrap();
        assert_eq!(events.status(), StatusCode::OK);
        events.chunk().await.unwrap();
        tabs.push(events);
    }
    assert_eq!(people().await, (0, 1), "one student, not one per tab");

    // The presentation view is not a student.
    let mut display = browser_as(&base, &idp, &teacher)
        .await
        .get(format!("{}/events", question.display_url))
        .send()
        .await
        .unwrap();
    display.chunk().await.unwrap();
    assert_eq!(people().await, (0, 1));

    // The student closes the page and is gone once the server notices.
    drop(tabs);
    let mut counts = people().await;
    for _ in 0..50 {
        if counts == (0, 0) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        counts = people().await;
    }
    assert_eq!(counts, (0, 0), "the closed page still counts");
}

#[tokio::test]
async fn the_teacher_follows_a_question_without_asking_again() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &["sojkam1", "novotny"]).await;
    let teacher = User::new("sojkam1", "Michal Sojka");
    let token = idp.access_token(&teacher, CLIENT_ID);
    let api = reqwest::Client::new();

    let question: QuestionInfo = api
        .post(format!("{base}/api/v1/questions"))
        .bearer_auth(&token)
        .json(&CreateQuestion {
            source_name: "question.md".to_owned(),
            markdown: QUESTION.to_owned(),
            view: StudentView::default(),
            shuffle: false,
            close_after: None,
            start_countdown: false,
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let url = format!("{base}/api/v1/questions/{}/events", question.id);

    // Only the teacher who asked the question may follow it.
    let other = idp.access_token(&User::new("novotny", "Jan Novotný"), CLIENT_ID);
    let response = api.get(&url).bearer_auth(&other).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = api.get(&url).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let mut events = api.get(&url).bearer_auth(&token).send().await.unwrap();
    assert_eq!(events.status(), StatusCode::OK);
    // Or nginx would hold the stream back.
    assert_eq!(events.headers()["x-accel-buffering"], "no");
    let mut buffer = String::new();

    // The current state comes right away.
    let first = next_question_event(&mut events, &mut buffer, |_| true).await;
    assert_eq!((first.answer_count, first.people, first.open), (0, 0, true));

    // A student opens the room...
    idp.login_as(&User::new("novakja1", "Jan Novák"));
    let student = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    student.get(format!("{base}/")).send().await.unwrap();
    let mut room = student
        .get(format!("{}/events", question.answer_url))
        .send()
        .await
        .unwrap();
    room.chunk().await.unwrap();
    next_question_event(&mut events, &mut buffer, |q| q.people == 1).await;

    // ...another one answers...
    answer_as(
        &base,
        &idp,
        &User::new("dvorakpe", "Petr Dvořák"),
        &question,
        1,
    )
    .await;
    next_question_event(&mut events, &mut buffer, |q| {
        (q.answer_count, q.people) == (1, 2)
    })
    .await;

    // ...the first one leaves...
    drop(room);
    next_question_event(&mut events, &mut buffer, |q| q.people == 1).await;

    // ...and the question is closed.
    let response = api
        .post(format!("{base}/api/v1/questions/{}/close", question.id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let closed = next_question_event(&mut events, &mut buffer, |q| !q.open).await;
    assert_eq!(closed.answer_count, 1);
}

#[tokio::test]
async fn a_login_cannot_be_finished_in_somebody_elses_browser() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &["sojkam1"]).await;
    let browser = || {
        reqwest::Client::builder()
            .cookie_store(true)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    };
    let location = |response: &reqwest::Response| {
        response.headers()[reqwest::header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned()
    };

    // The attacker starts a login and signs in as themselves, but stops
    // before their browser comes back to the server.
    idp.login_as(&User::new("podvodn1", "Petr Podvodník"));
    let attacker = browser();
    let to_provider = attacker.get(format!("{base}/login")).send().await.unwrap();
    let back = attacker.get(location(&to_provider)).send().await.unwrap();
    let callback = location(&back);
    assert!(callback.contains("/auth/callback?"), "{callback}");

    // Somebody else's browser is sent to the callback URL.
    let victim = browser();
    let response = victim.get(&callback).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = victim.get(format!("{base}/")).send().await.unwrap();
    assert!(
        location(&response).contains("/login"),
        "the victim is not logged in as the attacker"
    );

    // The browser that started the login finishes it all right, and the
    // cookie that bound it is gone afterwards.
    let response = attacker.get(&callback).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let cookies: Vec<_> = response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap().to_owned())
        .collect();
    assert!(
        cookies
            .iter()
            .any(|cookie| cookie.starts_with("quiz_login_") && cookie.contains("Max-Age=0")),
        "{cookies:?}"
    );
    let response = attacker.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.text().await.unwrap().contains("Petr Podvodník"));
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

    assert_eq!(
        question_on_page(&body).as_deref(),
        Some(question.id.as_str()),
        "the room shows the question that was just asked"
    );

    let response = browser
        .post(format!("{}/answer", question.answer_url))
        .form(&[
            ("question", question.id.as_str()),
            ("answer", answer.to_string().as_str()),
        ])
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
/// A browser with a session of `user`.
async fn browser_as(base: &str, idp: &MockIdp, user: &User) -> reqwest::Client {
    let browser = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    idp.login_as(user);
    let response = browser.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    browser
}

/// Reads the first server-sent event from `url`, as a logged-in browser.
async fn live_state_as(client: &reqwest::Client, url: &str) -> String {
    let mut response = client.get(url).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-accel-buffering"], "no");
    let chunk = tokio::time::timeout(Duration::from_secs(5), response.chunk())
        .await
        .expect("the event stream sends the state right away")
        .unwrap()
        .expect("the event stream is not empty");
    String::from_utf8_lossy(&chunk).into_owned()
}

/// Reads the teacher's event stream of a question until a state satisfies
/// `wanted`.
async fn next_question_event(
    events: &mut reqwest::Response,
    buffer: &mut String,
    wanted: impl Fn(&QuestionInfo) -> bool,
) -> QuestionInfo {
    let read = async {
        loop {
            while let Some(end) = buffer.find("\n\n") {
                let block: String = buffer.drain(..end + 2).collect();
                let Some(data) = block.lines().find_map(|line| line.strip_prefix("data: ")) else {
                    continue;
                };
                assert!(block.contains("event: state"), "{block}");
                let question: QuestionInfo = serde_json::from_str(data).unwrap();
                if wanted(&question) {
                    return question;
                }
            }
            let chunk = events.chunk().await.unwrap().expect("the stream goes on");
            buffer.push_str(std::str::from_utf8(&chunk).unwrap());
        }
    };
    tokio::time::timeout(Duration::from_secs(5), read)
        .await
        .expect("the expected state arrives")
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

#[tokio::test]
async fn metrics_count_questions_answers_and_who_is_connected() {
    let idp = MockIdp::start().await;
    let base = start_server(&idp, &["sojkam1"]).await;
    let teacher = User::new("sojkam1", "Michal Sojka");
    let token = idp.access_token(&teacher, CLIENT_ID);
    let api = reqwest::Client::new();

    let metrics = || {
        let request = api.get(format!("{base}/metrics"));
        async move {
            let response = request.send().await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            response.text().await.unwrap()
        }
    };
    let text = metrics().await;
    for line in [
        "quiz_questions_asked_total 0",
        "quiz_answers_total 0",
        "quiz_rooms_open 0",
        "quiz_teachers_connected 0",
        "quiz_students_connected 0",
    ] {
        assert!(text.lines().any(|l| l == line), "{line:?} not in\n{text}");
    }

    let question: QuestionInfo = api
        .post(format!("{base}/api/v1/questions"))
        .bearer_auth(&token)
        .json(&CreateQuestion {
            source_name: "otazky.md".to_owned(),
            markdown: QUESTION.to_owned(),
            view: StudentView::default(),
            shuffle: false,
            close_after: None,
            start_countdown: false,
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    answer_as(
        &base,
        &idp,
        &User::new("novakja1", "Jan Novák"),
        &question,
        0,
    )
    .await;
    answer_as(
        &base,
        &idp,
        &User::new("dvorape2", "Petr Dvořák"),
        &question,
        1,
    )
    .await;

    // One student keeps the room open.
    idp.login_as(&User::new("svobomar", "Marie Svobodová"));
    let student = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    student.get(format!("{base}/")).send().await.unwrap();
    let mut events = student
        .get(format!("{}/events", question.answer_url))
        .send()
        .await
        .unwrap();
    events.chunk().await.unwrap();

    let text = metrics().await;
    for line in [
        "quiz_questions_asked_total 1",
        "quiz_answers_total 2",
        "quiz_rooms_open 1",
        "quiz_teachers_connected 1",
        "quiz_students_connected 1",
    ] {
        assert!(text.lines().any(|l| l == line), "{line:?} not in\n{text}");
    }
}

/// Starts the server binary with `--state-file`, on `port`, and waits until it
/// answers.
#[cfg(unix)]
async fn spawn_server_process(
    idp: &MockIdp,
    port: u16,
    state_file: &std::path::Path,
) -> std::process::Child {
    let base = format!("http://127.0.0.1:{port}");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_quiz-server"))
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .args(["--public-url", &base])
        .args(["--oidc-issuer", &idp.issuer])
        .args(["--oidc-client-id", CLIENT_ID])
        .args(["--teachers", "sojkam1"])
        .arg("--state-file")
        .arg(state_file)
        .arg("--log-file")
        .arg(state_file.with_extension("log"))
        .spawn()
        .expect("the server binary starts");
    for _ in 0..100 {
        if reqwest::get(format!("{base}/healthz"))
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            return child;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("the quiz server did not start");
}

#[cfg(unix)]
fn terminate(mut child: std::process::Child) {
    let status = std::process::Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(child.wait().unwrap().success(), "the server stops cleanly");
}

#[cfg(unix)]
#[tokio::test]
async fn rooms_and_answers_survive_a_restart_with_a_state_file() {
    let idp = MockIdp::start().await;
    let directory = std::env::temp_dir().join(format!("quiz-state-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let state_file = directory.join("state.json");
    let _ = std::fs::remove_file(&state_file);
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let base = format!("http://127.0.0.1:{port}");
    let token = idp.access_token(&User::new("sojkam1", "Michal Sojka"), CLIENT_ID);
    let api = reqwest::Client::new();

    let server = spawn_server_process(&idp, port, &state_file).await;
    let question: QuestionInfo = api
        .post(format!("{base}/api/v1/questions"))
        .bearer_auth(&token)
        .json(&CreateQuestion {
            source_name: "otazky.md".to_owned(),
            markdown: QUESTION.to_owned(),
            view: StudentView::default(),
            shuffle: false,
            close_after: None,
            start_countdown: false,
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    answer_as(
        &base,
        &idp,
        &User::new("novakja1", "Jan Novák"),
        &question,
        1,
    )
    .await;
    terminate(server);
    assert!(state_file.exists(), "the state is saved on termination");

    let server = spawn_server_process(&idp, port, &state_file).await;
    let room = api
        .get(format!("{base}/api/v1/room"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json::<RoomStatus>()
        .await
        .unwrap()
        .room
        .expect("the room is back");
    assert_eq!(room.code, question.code);
    assert_eq!(room.answer_url, question.answer_url);
    assert_eq!(room.current.as_deref(), Some(question.id.as_str()));

    let results: Results = api
        .get(format!("{base}/api/v1/questions/{}/results", question.id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(results.rows.len(), 1);
    assert_eq!(results.rows[0].login, "novakja1");
    assert!(results.question.open);

    // Another student can still join with the code from before the restart.
    answer_as(
        &base,
        &idp,
        &User::new("dvorakpe", "Petr Dvořák"),
        &question,
        0,
    )
    .await;
    terminate(server);

    // A state file that cannot be read does not keep the server from
    // starting, it just starts from scratch.
    std::fs::write(&state_file, "not json").unwrap();
    let server = spawn_server_process(&idp, port, &state_file).await;
    let status: RoomStatus = api
        .get(format!("{base}/api/v1/room"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(status.room.is_none());
    terminate(server);
    let log = std::fs::read_to_string(state_file.with_extension("log")).unwrap();
    assert!(log.contains("starting from scratch"), "{log}");
    let _ = std::fs::remove_dir_all(&directory);
}
