# Quiz

A small web app for asking students a multiple-choice question during a
lecture, and for collecting their answers.

It has two parts:

* **`quiz-server`** — an HTTP server that shows the question to students,
  collects their answers and serves an API for the CLI. It keeps everything in
  memory; the only file it writes is its log.
* **`quiz`** — the teacher's command line tool. It reads a question from a
  Markdown file, publishes it on the server, opens the presentation view in a
  browser and, when the teacher closes the question, saves the collected
  answers as a CSV next to the Markdown file.

Everybody authenticates with **OIDC**: students in the browser, the teacher in
the CLI. The student-facing web pages are in Czech, everything else in English.

## How one question works

```
$ quiz --server https://quiz.osy.fel.cvut.cz ask cviceni/01regex/otazky.md
Consider the following BASH code: ...

  A) In Bash, every stage of a pipeline runs in its own subshell. ...
  B) It doesn't handle errors.
  C) It will work only in BASH 4.0 and newer.

Question code:     428173
Students answer at: https://quiz.osy.fel.cvut.cz/q/4f3c...c1
Presentation view:  https://quiz.osy.fel.cvut.cz/d/9a77...b4

The presentation view was opened in a browser.
Press Enter to close the question and save the answers (Ctrl-C works too).
  answers: 17
```

1. The CLI parses the Markdown file and shows what it understood, so a broken
   question is noticed before the students see it.
2. It logs the teacher in (once; the token is cached) and publishes the
   question over the API.
3. It opens the **presentation view** in a browser. That page shows the
   question, a QR code with the answer URL, the six-digit question code and a
   live count of the answers. If no browser can be opened, the URL is printed
   for opening by hand.
4. Students either scan the QR code or go to the main page, log in and type the
   code — both ways land on the same answer URL. They tap one of the answers and
   may correct it while the question is open.
5. Pressing Enter closes the question. Every open browser switches over to a
   bar graph with the results, and the CLI writes
   `otazky_2026-09-21_14-05-03.csv` next to `otazky.md`.

## The question format

A question is one Markdown file. Everything above the first horizontal rule
(`---`) is the question; the top-level list items below it are the possible
answers. `question.md` in this directory is a working example:

````markdown
Consider the following BASH code:

``` bash
sum=0
cat numbers.txt | while read -r num; do
    sum=$((sum + num))
done
echo "Total: $sum"
```

What's wrong with it?

---

- In Bash, every stage of a pipeline runs in its own subshell.
  Modifications to variables inside the `while` loop affect the
  subshell, not the parent shell environment.

- It doesn't handle errors.

- It will work only in BASH 4.0 and newer.
````

Details worth knowing:

* Answers are labelled `A`, `B`, `C`, … in the order they appear, and there may
  be at most 26 of them. At least two are required.
* An answer may span several lines and contain any inline Markdown; nested
  lists stay part of the answer they belong to.
* `---`, `***` and `___` all work as the separator, and a rule inside a fenced
  code block is ignored.
* If the file starts with a heading, it becomes the page title.
* `quiz parse otazky.md` shows how a file is parsed without contacting the
  server.

## Building

```sh
cargo build --release          # target/release/quiz-server, target/release/quiz
cargo test                     # unit tests plus end-to-end tests
```

Built and tested with Rust 1.98; edition 2024 needs at least 1.85. No C
libraries are needed: TLS is provided
by rustls, and the stylesheet, the script and the HTML templates are compiled
into the binaries.

With Nix: `nix build` builds both binaries, `nix develop` gives a shell with
the toolchain.

## Trying it out locally

The repository contains a **development identity provider** so that the whole
app can be tried out on one machine, with no credentials and nothing to
register. It speaks real OpenID Connect but asks for no password: you type a
login into a form and get a token for it. Never run it anywhere but on your own
machine.

Three terminals, from the directory of this README:

```sh
# 1. the development identity provider (http://127.0.0.1:9000)
cargo run -p quiz-testsupport --bin mock-idp

# 2. the quiz server (http://localhost:8080)
cargo run -p quiz-server -- \
    --oidc-issuer http://127.0.0.1:9000 \
    --oidc-client-id quiz \
    --teachers sojkam1

# 3. ask the example question
cargo run -p quiz-cli --bin quiz -- ask question.md
```

What happens then:

1. The CLI prints the parsed question and opens a browser for the login. The
   development provider asks *who are you?* — log in as `sojkam1`, the login
   passed to `--teachers`.
2. The CLI publishes the question and opens the presentation view: the
   question, a QR code, the six-digit code and a live counter. Add
   `--no-browser` if you would rather open the printed URLs yourself.
3. To act as a student, open <http://localhost:8080/> **in a private window**
   (so it gets a session of its own), log in as somebody else, for example
   `novakja1`, type the code and pick an answer. The counter in the
   presentation view and in terminal 3 goes up immediately. Repeat in further
   private windows for more students.
4. Press Enter in terminal 3. The question closes, every open page switches to
   the bar graph, and `question_<date>_<time>.csv` appears next to
   `question.md`.

Worth knowing:

* Use `http://localhost:8080` consistently. The session cookie belongs to the
  host name it was issued for, so mixing `localhost` and `127.0.0.1` means
  logging in twice.
* `quiz logout` forgets the cached teacher token, which is how you log in as
  somebody else on the next `quiz ask`.
* The server logs everything to `quiz-server.log` in the current directory, so
  `tail -f quiz-server.log` is a good fourth terminal.
* To scan the QR code with a phone, everything has to be reachable from the
  phone, so use the address of your machine on the network instead of
  localhost:

  ```sh
  cargo run -p quiz-testsupport --bin mock-idp -- --listen 192.168.1.5:9000
  cargo run -p quiz-server -- --listen 0.0.0.0:8080 \
      --public-url http://192.168.1.5:8080 \
      --oidc-issuer http://192.168.1.5:9000 \
      --oidc-client-id quiz --teachers sojkam1
  ```

To try the same walk-through against a real identity provider instead, run for
example Keycloak in development mode, create a client as described in the next
section, and point `--oidc-issuer` at
`http://localhost:8081/realms/<your realm>`.

## Registering the OIDC client

The server needs one OIDC client at your identity provider (Keycloak, the CTU
SSO, …):

| Setting | Value |
| --- | --- |
| Client ID | e.g. `quiz`, passed as `--oidc-client-id` |
| Redirect URI | `<public URL>/auth/callback` |
| Grant type | authorization code, with PKCE |
| Scopes | `openid profile email` |
| Client secret | optional; leave unset for a public client |

The CLI can reuse the same client (it is then used as a public client with a
loopback redirect URI) or get one of its own via `--oidc-cli-client-id`. Its
redirect URI is `http://127.0.0.1:<port>/callback`. Providers that follow
RFC 8252 accept any loopback port; for stricter ones, register
`http://127.0.0.1:8765/callback` and set `--oidc-cli-redirect-uri`. The CLI
asks the server which client to use, so teachers only ever configure
`--server`.

The login name of a user is taken from the `preferred_username` claim, which is
the familiar eight-character login (`sojkam1`) at CTU. Use `--login-claim
email` or `--login-claim subject` if your provider is different.

## Running the server

```sh
quiz-server \
    --listen 127.0.0.1:8080 \
    --public-url https://quiz.osy.fel.cvut.cz \
    --oidc-issuer https://idp.example.org/realms/fel \
    --oidc-client-id quiz \
    --teachers sojkam1,vlasakji
```

Every option has an environment variable, which is what the systemd unit and
the container image use (`quiz-server --help` lists them all):

| Option | Variable | Default |
| --- | --- | --- |
| `--listen` | `QUIZ_LISTEN` | `127.0.0.1:8080` |
| `--public-url` | `QUIZ_PUBLIC_URL` | `http://localhost:8080` |
| `--log-file` | `QUIZ_LOG_FILE` | `quiz-server.log` |
| `--log-filter` | `QUIZ_LOG_FILTER` | `info` (`RUST_LOG` wins) |
| `--oidc-issuer` | `QUIZ_OIDC_ISSUER` | required |
| `--oidc-client-id` | `QUIZ_OIDC_CLIENT_ID` | required |
| `--oidc-client-secret` | `QUIZ_OIDC_CLIENT_SECRET` | none (public client) |
| `--oidc-cli-client-id` | `QUIZ_OIDC_CLI_CLIENT_ID` | same as the server's |
| `--oidc-cli-redirect-uri` | `QUIZ_OIDC_CLI_REDIRECT_URI` | any loopback port |
| `--oidc-scopes` | `QUIZ_OIDC_SCOPES` | `openid,profile,email` |
| `--oidc-audiences` | `QUIZ_OIDC_AUDIENCES` | none |
| `--teachers` | `QUIZ_TEACHERS` | empty: everybody may ask |
| `--login-claim` | `QUIZ_LOGIN_CLAIM` | `preferred-username` |
| `--question-ttl-hours` | `QUIZ_QUESTION_TTL_HOURS` | `12` |
| `--session-ttl-hours` | `QUIZ_SESSION_TTL_HOURS` | `12` |
| `--trust-proxy` | `QUIZ_TRUST_PROXY` | off |

`--public-url` may contain a path prefix (`https://osy.fel.cvut.cz/quiz`); the
server then serves all its pages under that prefix, so the reverse proxy has to
forward the path unchanged — see `deploy/nginx.conf.example`.

## Teacher CLI

```sh
quiz ask otazky.md            # publish, present, collect, save (the main one)
quiz ask otazky.md -o answers.csv --close-after 120
quiz parse otazky.md          # check the Markdown, no server needed
quiz login                    # log in ahead of the lecture
quiz whoami                    # who am I, and may I ask questions?
quiz logout [--all]
quiz close <question id>      # if `quiz ask` was interrupted: close and save
```

`--server` (or `QUIZ_SERVER`) selects the server, `--no-browser` never opens a
browser and only prints URLs, `--login-port` sets the port of the local
listener used during the login. The ID token is cached in
`$XDG_CONFIG_HOME/quiz/tokens.json` (mode 0600), so the browser is normally
only needed once a day.

Ctrl-C does the same as Enter: the question is closed and the answers are
saved, so a mistyped key does not lose the collected data.

## The CSV file

One row per student, sorted by login:

```csv
login,name,answer,answer_text,answered_at
novakja1,Jan Novák,B,It doesn't handle errors.,2026-09-21T12:05:41.881Z
svobodpe,Petra Svobodová,C,It will work only in BASH 4.0 and newer.,2026-09-21T12:05:44.102Z
```

The file name is the base name of the question file plus the date and time:
`otazky.md` → `otazky_2026-09-21_14-05-03.csv`, written into the same
directory unless `--output` says otherwise.

## Deployment

The server speaks plain HTTP and expects a reverse proxy in front of it for
TLS. Ready-made pieces:

* `deploy/nginx.conf.example` — TLS, forwarded headers and the unbuffered
  proxying that the event streams need.
* `deploy/quiz-server.service` and `deploy/quiz-server.env.example` — a
  hardened systemd unit using `DynamicUser`, with the log in
  `/var/log/quiz-server`.
* `Dockerfile` and `docker-compose.yml` — a two-stage build producing an image
  with both binaries.
* `flake.nix` — `nix build`, `nix develop`, and a NixOS module:

  ```nix
  {
    imports = [ inputs.quiz.nixosModules.default ];
    services.quiz-server = {
      enable = true;
      publicUrl = "https://quiz.osy.fel.cvut.cz";
      issuer = "https://idp.example.org/realms/fel";
      teachers = [ "sojkam1" ];
      environmentFile = "/run/secrets/quiz-server.env";  # the client secret
    };
  }
  ```

Restarting the server is cheap: nothing is stored on disc, so only questions
that are open at that moment are lost, and browser sessions have to log in
again (the cookie signing key is new).

## Logging

All operations go to the log file **and** to the standard error output: logins,
created questions (with code and URLs), every recorded answer, closing a
question, exported results, refused tokens and one line per HTTP request.

```
2026-09-21T12:04:58.402Z  INFO http{method=POST uri=/api/v1/questions}: quiz_server::api:
    question created question=4f3c…c1 code=428173 teacher=sojkam1 source=otazky.md answers=3
2026-09-21T12:05:41.881Z  INFO http{method=POST uri=/q/4f3c…c1/answer}: quiz_server::web:
    answer recorded question=4f3c…c1 login=novakja1 answer=B changed=false total=12
```

Use `--log-filter` or `RUST_LOG` (e.g. `RUST_LOG=info,quiz_server=debug`) to
change the verbosity. The log is append-only and never read back by the server.

## Security notes

* Every student-facing page and every API endpoint requires an OIDC login. The
  only exceptions are `/healthz`, the two static files and
  `/api/v1/oidc`, which just tells the CLI where to log in.
* The **presentation view** is a capability URL: `/d/<32 hex characters>`. It
  is handed to the teacher over the authenticated API and needs no login, so
  nothing has to be typed on the machine driving the projector. Keep it
  unshared — anybody with the URL can watch the question, though not answer it
  or close it.
* Per-answer counts are only published once the question is closed, so students
  cannot watch the distribution while they are answering.
* Session cookies are signed with a key generated at startup, `HttpOnly`,
  `SameSite=Lax` and, when the public URL is HTTPS, `Secure`.
* ID tokens presented by the CLI are verified against the provider's signing
  keys, with issuer, audience and expiry checked.
* Only logins on the `--teachers` list may ask questions, and a question can
  only be closed or exported by the teacher who asked it.

## Where each requirement is implemented

| # | Requirement | Implementation |
| --- | --- | --- |
| 1 | HTTP server plus a CLI for teachers | `quiz-server` (`server/`) and `quiz` (`cli/`) |
| 2 | Server reachable under a public URL | `--public-url`, plus `deploy/` for the reverse proxy, systemd, Docker and NixOS |
| 3 | The CLI talks to the server over an HTTP API | `common/src/api.rs`, `cli/src/client.rs`, routes under `/api/v1` |
| 4 | No state on disc | `server/src/state.rs` keeps questions, answers and sessions in memory only |
| 5 | All operations logged to a file by a logging framework | `tracing` with a file and a stderr layer, set up in `init_logging` (`server/src/lib.rs`) |
| 6 | Authorization via OIDC | `server/src/auth.rs` (browser sessions, ID-token checks), `cli/src/auth.rs` (loopback login) |
| 7 | Question read from Markdown, shown in a browser, URL printed otherwise | `cli/src/main.rs`: `ask` and `show_in_browser` |
| 8 | Answers are the top-level list items after `---` | `common/src/question.rs` |
| 9 | QR code with the answer URL and a random question code | `server/src/display.rs` (`qr_svg`), code generated in `state.rs` |
| 10 | Main page asks for the code and redirects to the QR URL | `server/src/web.rs`: `index` and `join` |
| 11 | Live update of the number of answers | `server/src/live.rs` (server-sent events) and `server/static/app.js` |
| 12 | Closing shows a bar graph and writes a CSV of logins and answers | `api::close_question`, `templates/results.html`, `cli/src/report.rs` |

## How the code is organized

| Path | What is in it |
| --- | --- |
| `common/src/question.rs` | parsing a question from Markdown |
| `common/src/api.rs` | the JSON types of the HTTP API |
| `server/src/config.rs` | command line and environment options |
| `server/src/auth.rs` | OIDC: login in the browser, token checks for the CLI |
| `server/src/state.rs` | the in-memory questions, answers and sessions |
| `server/src/web.rs` | the student pages |
| `server/src/display.rs` | the presentation view and the QR code |
| `server/src/live.rs` | the server-sent event streams |
| `server/src/api.rs` | the API used by the CLI |
| `server/templates`, `server/static` | HTML, CSS and the live-update script |
| `cli/src/auth.rs` | the loopback OIDC login and the token cache |
| `cli/src/report.rs` | terminal output and the CSV export |
| `testsupport/` | mock OIDC provider: the tests' fixture and the `mock-idp` binary |

## Tests

```sh
cargo test                                  # everything
cargo test -p quiz-common                   # the Markdown parser
cargo test -p quiz-server --test e2e        # a whole question, over HTTP
cargo test -p quiz-cli --test cli           # drives the compiled binary
```

The tests need no credentials and no network: `testsupport` starts a mock OIDC
provider that serves a discovery document and a JWKS and signs real RS256 ID
tokens, so the actual OIDC code paths are exercised. `cargo test -p quiz-cli`
runs the real `quiz` binary, including the loopback login and the CSV export.

`QUIZ_DUMP_PAGES=/tmp/pages cargo test -p quiz-server --test e2e` additionally
saves the rendered pages, which is a quick way to look at the styling.

## Troubleshooting

| Symptom | Cause |
| --- | --- |
| `OIDC discovery at … failed` at startup | wrong `--oidc-issuer`, or the provider is unreachable; the issuer must be the URL *without* `/.well-known/openid-configuration` |
| Login ends with `redirect_uri` mismatch | `<public URL>/auth/callback` is not registered with the provider |
| `not logged in: the ID token is not valid` | the cached token expired and could not be refreshed — `quiz login` |
| `not allowed: … is not allowed to ask questions` | the login is missing from `--teachers` |
| Asked to log in again and again, or the code form comes back after submitting | the server is being reached under a different host name than `--public-url`, so the session cookie does not come back |
| The answer count does not move | the reverse proxy buffers the event streams; see `deploy/nginx.conf.example` |
| The QR code leads nowhere | `--public-url` does not match how students reach the server |
| ``no `---` separator found`` | the Markdown file has no horizontal rule before the answers |
