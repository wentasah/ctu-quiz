//! A mock OIDC provider and a helper for starting the quiz server under test.
//!
//! This crate is a development dependency of `quiz-server` and `quiz-cli`; it
//! is never part of a release build, which is why it may carry a private key.
//!
//! The mock speaks enough of OpenID Connect for the real client code to work:
//! it serves a discovery document and a JWKS, redirects `/authorize` straight
//! back to the application (as if the user had just logged in) and signs real
//! RS256 ID tokens and access tokens at `/token`. That way the tests exercise
//! the actual OIDC code paths without needing credentials for a real identity
//! provider.
//!
//! The access tokens are JWTs shaped like those of Keycloak: they carry the
//! user's profile claims, and both kinds of token say what they are in `typ`.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Form, Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use chrono::{TimeDelta, Utc};
use clap::Parser;
use openidconnect::{
    AccessToken, AdditionalClaims, Audience, AuthUrl, EmptyAdditionalProviderMetadata,
    EmptyExtraTokenFields, EndUserEmail, EndUserName, EndUserUsername, IdToken, IdTokenClaims,
    IdTokenFields, IssuerUrl, JsonWebKeyId, JsonWebKeySetUrl, LocalizedClaim, Nonce,
    PrivateSigningKey, RefreshToken, ResponseTypes, Scope, StandardClaims, StandardTokenResponse,
    SubjectIdentifier, TokenUrl,
    core::{
        CoreClientAuthMethod, CoreGenderClaim, CoreJsonWebKeySet,
        CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm, CoreProviderMetadata,
        CoreResponseType, CoreRsaPrivateSigningKey, CoreSubjectIdentifierType, CoreTokenType,
    },
};
use quiz_server::config::Config;
use tokio::net::TcpListener;
use uuid::Uuid;

/// Key used to sign the tokens of the mock provider. Test fixture only.
const SIGNING_KEY: &str = include_str!("test-key.pem");

/// The example question, used by the tests of all the crates.
pub const EXAMPLE_QUESTION: &str = include_str!("../question.md");

/// The same question written as a script: a shebang line, and the file made
/// executable, so that running it asks the question.
pub const EXECUTABLE_QUESTION: &str = include_str!("../executable.md");

/// A question whose answers carry no points.
pub const UNSCORED_QUESTION: &str = include_str!("../unscored.md");

/// The question id that the answer form of a room page carries, which a
/// browser submits together with the chosen answer.
pub fn question_on_page(html: &str) -> Option<String> {
    let rest = html.split_once("name=\"question\" value=\"")?.1;
    Some(rest.split_once('"')?.0.to_owned())
}

/// How the mock provider decides who is logging in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginMode {
    /// Log in as the user set with [`MockIdp::login_as`], without asking.
    /// This is what the automated tests use.
    Automatic,
    /// Ask in the browser, with a form that accepts any login. This is what
    /// the `mock-idp` binary uses for trying the app out by hand.
    Interactive,
}

/// Somebody the mock provider can log in as.
#[derive(Debug, Clone)]
pub struct User {
    pub sub: String,
    pub login: String,
    pub name: String,
    pub email: String,
}

impl User {
    pub fn new(login: &str, name: &str) -> Self {
        Self {
            sub: format!("sub-{login}"),
            login: login.to_owned(),
            name: name.to_owned(),
            email: format!("{login}@example.org"),
        }
    }
}

/// The `typ` claim with which Keycloak tells its tokens apart.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TokenType {
    typ: String,
}

impl AdditionalClaims for TokenType {}

/// A signed token of the mock. Access tokens have the claims of an ID token
/// too, so one type does for both.
type Jwt =
    IdToken<TokenType, CoreGenderClaim, CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm>;

/// An ID token is `ID`, an access token `Bearer`.
const ID_TOKEN: &str = "ID";
const ACCESS_TOKEN: &str = "Bearer";

/// An authorization code waiting to be exchanged, or a refresh token.
#[derive(Clone)]
struct Pending {
    user: User,
    nonce: Option<String>,
    client_id: String,
}

struct Inner {
    issuer: String,
    mode: LoginMode,
    key: CoreRsaPrivateSigningKey,
    jwks: CoreJsonWebKeySet,
    /// Who the next visit of `/authorize` logs in as.
    next_user: Mutex<User>,
    pending: Mutex<HashMap<String, Pending>>,
    /// Refresh tokens handed out, and whom they log in again.
    refresh_tokens: Mutex<HashMap<String, Pending>>,
    /// How long the access tokens live, as far as `expires_in` says.
    token_lifetime: Mutex<Duration>,
    refreshes: AtomicUsize,
}

/// A running mock OIDC provider.
pub struct MockIdp {
    inner: Arc<Inner>,
    pub issuer: String,
}

impl MockIdp {
    /// Starts a provider that logs everybody in without asking, on a port
    /// picked by the operating system.
    pub async fn start() -> Self {
        Self::listen("127.0.0.1:0", LoginMode::Automatic)
            .await
            .expect("a random loopback port is available")
    }

    /// Starts a provider on `address`. With [`LoginMode::Interactive`] it asks
    /// in the browser who is logging in.
    pub async fn listen(address: &str, mode: LoginMode) -> std::io::Result<Self> {
        let listener = TcpListener::bind(address).await?;
        let issuer = format!("http://{}", listener.local_addr()?);

        let key = CoreRsaPrivateSigningKey::from_pem(
            SIGNING_KEY,
            Some(JsonWebKeyId::new("test-key".to_owned())),
        )
        .expect("the test signing key is a valid PKCS#1 PEM");
        let jwks = CoreJsonWebKeySet::new(vec![key.as_verification_key()]);

        let inner = Arc::new(Inner {
            issuer: issuer.clone(),
            mode,
            key,
            jwks,
            next_user: Mutex::new(User::new("nobody", "Nobody")),
            pending: Mutex::new(HashMap::new()),
            refresh_tokens: Mutex::new(HashMap::new()),
            token_lifetime: Mutex::new(Duration::from_secs(3600)),
            refreshes: AtomicUsize::new(0),
        });

        let app = Router::new()
            .route("/.well-known/openid-configuration", get(metadata))
            .route("/jwks", get(keys))
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .with_state(inner.clone());

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        Ok(Self { inner, issuer })
    }

    /// Makes the next login in a browser happen as `user`.
    pub fn login_as(&self, user: &User) {
        *self.inner.next_user.lock().unwrap() = user.clone();
    }

    /// Makes the token responses say that the access tokens expire after
    /// `lifetime`. The tokens themselves stay valid for an hour, so that the
    /// server accepts them while the client already wants new ones.
    pub fn set_token_lifetime(&self, lifetime: Duration) {
        *self.inner.token_lifetime.lock().unwrap() = lifetime;
    }

    /// How many times a refresh token has been used.
    pub fn refreshes(&self) -> usize {
        self.inner.refreshes.load(Ordering::SeqCst)
    }

    /// Mints an access token for `user` with the API `audience`, as the CLI
    /// would obtain it.
    pub fn access_token(&self, user: &User, audience: &str) -> String {
        sign(&self.inner, user, audience, ACCESS_TOKEN, None, None).to_string()
    }

    /// Mints an access token that expired an hour ago.
    pub fn expired_access_token(&self, user: &User, audience: &str) -> String {
        let claims = claims(&self.inner, user, audience, ACCESS_TOKEN)
            .set_expiration(Utc::now() - TimeDelta::hours(1))
            .set_issue_time(Utc::now() - TimeDelta::hours(2));
        Jwt::new(
            claims,
            &self.inner.key,
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
            None,
            None,
        )
        .unwrap()
        .to_string()
    }

    /// Mints an ID token for `user`, which the CLI keeps to itself.
    pub fn id_token(&self, user: &User, audience: &str) -> String {
        sign(&self.inner, user, audience, ID_TOKEN, None, None).to_string()
    }
}

async fn metadata(State(inner): State<Arc<Inner>>) -> Json<CoreProviderMetadata> {
    let issuer = &inner.issuer;
    Json(
        CoreProviderMetadata::new(
            IssuerUrl::new(issuer.clone()).unwrap(),
            AuthUrl::new(format!("{issuer}/authorize")).unwrap(),
            JsonWebKeySetUrl::new(format!("{issuer}/jwks")).unwrap(),
            vec![ResponseTypes::new(vec![CoreResponseType::Code])],
            vec![CoreSubjectIdentifierType::Public],
            vec![CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256],
            EmptyAdditionalProviderMetadata {},
        )
        .set_token_endpoint(Some(TokenUrl::new(format!("{issuer}/token")).unwrap()))
        .set_scopes_supported(Some(
            ["openid", "profile", "email"]
                .into_iter()
                .map(|scope| Scope::new(scope.to_owned()))
                .collect(),
        ))
        .set_token_endpoint_auth_methods_supported(Some(vec![
            CoreClientAuthMethod::None,
            CoreClientAuthMethod::ClientSecretPost,
            CoreClientAuthMethod::ClientSecretBasic,
        ])),
    )
}

async fn keys(State(inner): State<Arc<Inner>>) -> Json<CoreJsonWebKeySet> {
    Json(inner.jwks.clone())
}

#[derive(serde::Deserialize)]
struct AuthorizeQuery {
    client_id: String,
    redirect_uri: String,
    state: String,
    nonce: Option<String>,
    /// Filled in by the login form below, in interactive mode.
    login: Option<String>,
    name: Option<String>,
}

/// Logs the configured user in without asking anything.
async fn authorize(
    State(inner): State<Arc<Inner>>,
    Query(query): Query<AuthorizeQuery>,
) -> Response {
    let user = match inner.mode {
        LoginMode::Automatic => inner.next_user.lock().unwrap().clone(),
        LoginMode::Interactive => match query.login.as_deref().map(str::trim) {
            Some(login) if !login.is_empty() => {
                let name = query
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .unwrap_or(login);
                User::new(login, name)
            }
            // Nobody said who they are yet, so ask.
            _ => return Html(login_form(&query)).into_response(),
        },
    };

    let code = Uuid::new_v4().simple().to_string();
    inner.pending.lock().unwrap().insert(
        code.clone(),
        Pending {
            user,
            nonce: query.nonce,
            client_id: query.client_id,
        },
    );

    let separator = if query.redirect_uri.contains('?') {
        '&'
    } else {
        '?'
    };
    let parameters = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("code", &code)
        .append_pair("state", &query.state)
        .finish();
    Redirect::to(&format!("{}{separator}{parameters}", query.redirect_uri)).into_response()
}

/// The login form of the interactive mode: type any login, get a token.
fn login_form(query: &AuthorizeQuery) -> String {
    let hidden = |name: &str, value: &str| {
        format!(
            "<input type=\"hidden\" name=\"{name}\" value=\"{}\">",
            escape(value)
        )
    };
    let nonce = match &query.nonce {
        Some(nonce) => hidden("nonce", nonce),
        None => String::new(),
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Development identity provider</title>
  <style>
    body {{ font: 16px/1.5 system-ui, sans-serif; margin: 3rem auto; max-width: 28rem;
           padding: 0 1rem; }}
    .warning {{ background: #ffe8e6; border: 1px solid #b3261e; color: #7a1a15;
                border-radius: 8px; padding: .75rem 1rem; }}
    label {{ display: block; margin: 1rem 0; }}
    input[type=text] {{ font: inherit; width: 100%; padding: .5rem;
                        border: 1px solid #aaa; border-radius: 6px; }}
    button {{ font: inherit; padding: .6rem 1.2rem; border-radius: 6px;
              border: 1px solid #0b5fff; background: #0b5fff; color: #fff; }}
    .hint {{ color: #555; font-size: .9rem; }}
  </style>
</head>
<body>
  <p class="warning"><strong>Development identity provider.</strong>
    It signs a token for whatever login you type, without any password.
    Never run it anywhere but on your own machine.</p>
  <h1>Who are you?</h1>
  <form method="get" action="/authorize">
    {client_id}{redirect_uri}{state}{nonce}
    <label>Login
      <input type="text" name="login" value="sojkam1" autofocus
             autocapitalize="off" autocomplete="off"></label>
    <label>Full name
      <input type="text" name="name" value="Michal Sojka"></label>
    <button type="submit">Log in</button>
  </form>
  <p class="hint">Log in as the teacher (the login passed to
    <code>--teachers</code>) to ask questions, or as anybody else, for example
    <code>novakja1</code>, to answer them as a student.</p>
  <p class="hint">Client: <code>{client}</code></p>
</body>
</html>
"#,
        client_id = hidden("client_id", &query.client_id),
        redirect_uri = hidden("redirect_uri", &query.redirect_uri),
        state = hidden("state", &query.state),
        client = escape(&query.client_id),
    )
}

/// Enough escaping for attribute values built from query parameters.
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[derive(serde::Deserialize)]
struct TokenForm {
    grant_type: Option<String>,
    code: Option<String>,
    refresh_token: Option<String>,
    client_id: Option<String>,
}

async fn token(State(inner): State<Arc<Inner>>, Form(form): Form<TokenForm>) -> Response {
    let pending = if form.grant_type.as_deref() == Some("refresh_token") {
        inner.refreshes.fetch_add(1, Ordering::SeqCst);
        form.refresh_token
            .and_then(|token| inner.refresh_tokens.lock().unwrap().remove(&token))
            // A refreshed ID token carries no nonce.
            .map(|pending| Pending {
                nonce: None,
                ..pending
            })
    } else {
        form.code
            .and_then(|code| inner.pending.lock().unwrap().remove(&code))
    };
    let Some(pending) = pending else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_grant"})),
        )
            .into_response();
    };

    // The client and the API are the same in the tests, so the client ID
    // makes a fine audience for the access token too.
    let audience = form.client_id.unwrap_or_else(|| pending.client_id.clone());
    let access_token = AccessToken::new(
        sign(&inner, &pending.user, &audience, ACCESS_TOKEN, None, None).to_string(),
    );
    let id_token = sign(
        &inner,
        &pending.user,
        &audience,
        ID_TOKEN,
        pending.nonce.as_deref(),
        Some(&access_token),
    );

    let mut response = StandardTokenResponse::new(
        access_token,
        CoreTokenType::Bearer,
        IdTokenFields::new(Some(id_token), EmptyExtraTokenFields {}),
    );
    response.set_expires_in(Some(&inner.token_lifetime.lock().unwrap()));

    let refresh_token = Uuid::new_v4().simple().to_string();
    inner
        .refresh_tokens
        .lock()
        .unwrap()
        .insert(refresh_token.clone(), pending);
    response.set_refresh_token(Some(RefreshToken::new(refresh_token)));
    Json(response).into_response()
}

fn claims(
    inner: &Inner,
    user: &User,
    audience: &str,
    typ: &str,
) -> IdTokenClaims<TokenType, CoreGenderClaim> {
    IdTokenClaims::new(
        IssuerUrl::new(inner.issuer.clone()).unwrap(),
        vec![Audience::new(audience.to_owned())],
        Utc::now() + TimeDelta::hours(1),
        Utc::now(),
        StandardClaims::new(SubjectIdentifier::new(user.sub.clone()))
            .set_preferred_username(Some(EndUserUsername::new(user.login.clone())))
            .set_name(Some(LocalizedClaim::from(EndUserName::new(
                user.name.clone(),
            ))))
            .set_email(Some(EndUserEmail::new(user.email.clone()))),
        TokenType {
            typ: typ.to_owned(),
        },
    )
}

fn sign(
    inner: &Inner,
    user: &User,
    audience: &str,
    typ: &str,
    nonce: Option<&str>,
    access_token: Option<&AccessToken>,
) -> Jwt {
    let mut claims = claims(inner, user, audience, typ);
    if let Some(nonce) = nonce {
        claims = claims.set_nonce(Some(Nonce::new(nonce.to_owned())));
    }
    Jwt::new(
        claims,
        &inner.key,
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
        access_token,
        None,
    )
    .expect("signing a token works")
}

/// The client ID both the web UI and the CLI use in the tests.
pub const CLIENT_ID: &str = "quiz-app";

/// The log file all servers of one test binary write to.
pub fn log_file() -> PathBuf {
    std::env::temp_dir().join(format!("quiz-test-{}.log", std::process::id()))
}

/// Sets up logging once per test binary, and keeps the flushing guard alive.
fn init_logging_once(cfg: &Config) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = std::fs::remove_file(log_file());
        let guard = quiz_server::init_logging(cfg).expect("logging can be set up");
        std::mem::forget(guard);
    });
}

/// Starts the quiz server on a random port and returns its base URL.
///
/// An empty `teachers` list means every login may ask questions, which the
/// server only does when it is told to explicitly.
pub async fn start_server(idp: &MockIdp, teachers: &[&str]) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());

    // Go through the real command line parser, so that the tests cover it too.
    let mut args = vec![
        "quiz-server".to_owned(),
        "--public-url".to_owned(),
        base.clone(),
        "--oidc-issuer".to_owned(),
        idp.issuer.clone(),
        "--oidc-client-id".to_owned(),
        CLIENT_ID.to_owned(),
        "--teachers".to_owned(),
        teachers.join(","),
        "--log-file".to_owned(),
        log_file().display().to_string(),
    ];
    if teachers.is_empty() {
        args.push("--allow-any-teacher".to_owned());
    }
    let cfg = Config::parse_from(args).normalize().unwrap();

    init_logging_once(&cfg);

    tokio::spawn(async move {
        quiz_server::serve(cfg, listener).await.unwrap();
    });

    // Wait until the server answers, so the tests do not race the startup.
    let client = reqwest::Client::new();
    for _ in 0..100 {
        if client
            .get(format!("{base}/healthz"))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            return base;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the quiz server did not start");
}
