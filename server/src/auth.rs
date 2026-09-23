//! Authentication and authorization, all of it based on OIDC.
//!
//! Students log in in the browser with the authorization code flow and get a
//! signed session cookie. Teachers log in with the CLI, which performs the same
//! flow on a loopback address and then sends the resulting ID token in the
//! `Authorization: Bearer` header; [`Oidc::verify_id_token`] validates it
//! against the provider's signing keys.

use anyhow::{Context, Result, anyhow, bail};
use axum::{
    extract::{FromRequestParts, Query, State},
    http::{header, request::Parts},
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::{Cookie, Key, SameSite, SignedCookieJar};
use chrono::Utc;
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, IssuerUrl, Nonce, NonceVerifier, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl,
    Scope, TokenResponse,
    core::{
        CoreAuthenticationFlow, CoreClient, CoreIdToken, CoreIdTokenClaims, CoreIdTokenVerifier,
        CoreJsonWebKeySet, CoreProviderMetadata,
    },
};
use serde::Deserialize;
use url::Url;

use crate::{
    config::{Config, LoginClaim},
    error::{ApiFailure, PageError},
    state::{AppState, PendingLogin},
    views,
};

/// Name of the cookie holding the (signed) session id.
pub const SESSION_COOKIE: &str = "quiz_session";

/// The OIDC client, with the endpoints discovered from the provider.
type DiscoveredClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

/// Who somebody is, as far as the quiz app is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// Login name, `preferred_username` by default.
    pub login: String,
    /// Full name, or the login again if the provider does not give one.
    pub name: String,
}

/// Accepts any nonce, or none at all.
///
/// The nonce of an ID token presented by the CLI was chosen by the CLI, so the
/// server cannot check it. The token's signature, issuer, audience and validity
/// are checked, which is what authenticates the teacher.
struct AnyNonce;

impl NonceVerifier for AnyNonce {
    fn verify(self, _nonce: Option<&Nonce>) -> Result<(), String> {
        Ok(())
    }
}

/// Everything needed to talk to the OIDC provider.
pub struct Oidc {
    client: DiscoveredClient,
    http: reqwest::Client,
    issuer: IssuerUrl,
    jwks: CoreJsonWebKeySet,
    /// Client ID the CLI authenticates with.
    cli_client_id: ClientId,
    /// Further audiences accepted in ID tokens.
    audiences: Vec<String>,
    scopes: Vec<Scope>,
    login_claim: LoginClaim,
}

impl Oidc {
    /// Discovers the provider's endpoints and keys. Called once at startup, so
    /// that a misconfigured provider is noticed immediately.
    pub async fn discover(cfg: &Config) -> Result<Self> {
        // Redirects are disabled to limit the damage a malicious discovery
        // document could do, as recommended by the openidconnect crate.
        let http = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("building the HTTP client for OIDC failed")?;

        // Issuer identifiers are compared verbatim and are conventionally
        // written without a trailing slash, which `Url` likes to add.
        let issuer = IssuerUrl::new(cfg.oidc_issuer.as_str().trim_end_matches('/').to_owned())
            .context("invalid --oidc-issuer")?;

        let metadata = CoreProviderMetadata::discover_async(issuer.clone(), &http)
            .await
            .with_context(|| format!("OIDC discovery at {} failed", issuer.as_str()))?;

        let jwks = metadata.jwks().clone();
        let redirect_uri = RedirectUrl::new(cfg.redirect_uri()?.to_string())
            .context("--public-url does not yield a valid redirect URI")?;
        let client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(cfg.oidc_client_id.clone()),
            cfg.oidc_client_secret.clone().map(ClientSecret::new),
        )
        .set_redirect_uri(redirect_uri);

        Ok(Self {
            client,
            http,
            issuer,
            jwks,
            cli_client_id: ClientId::new(cfg.cli_client_id().to_owned()),
            audiences: cfg.oidc_audiences.clone(),
            // "openid" is always sent by the crate itself.
            scopes: cfg
                .oidc_scopes
                .iter()
                .filter(|s| *s != "openid")
                .map(|s| Scope::new(s.clone()))
                .collect(),
            login_claim: cfg.login_claim,
        })
    }

    /// Starts a login: the URL to send the browser to, plus the state that has
    /// to be remembered until the provider redirects back.
    pub fn authorize_url(&self) -> (Url, CsrfToken, Nonce, PkceCodeVerifier) {
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut request = self.client.authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        );
        for scope in &self.scopes {
            request = request.add_scope(scope.clone());
        }
        let (url, csrf, nonce) = request.set_pkce_challenge(challenge).url();
        (url, csrf, nonce, verifier)
    }

    /// Finishes a login started with [`Oidc::authorize_url`].
    pub async fn exchange_code(
        &self,
        code: String,
        pkce_verifier: PkceCodeVerifier,
        nonce: &Nonce,
    ) -> Result<Identity> {
        let response = self
            .client
            .exchange_code(AuthorizationCode::new(code))
            .context("the provider has no token endpoint")?
            .set_pkce_verifier(pkce_verifier)
            .request_async(&self.http)
            .await
            .context("exchanging the authorization code failed")?;

        let id_token = response
            .id_token()
            .ok_or_else(|| anyhow!("the provider returned no ID token"))?;
        let claims = id_token
            .claims(&self.client.id_token_verifier(), nonce)
            .context("the ID token is not valid")?;

        Ok(self.identity(claims))
    }

    /// Validates an ID token presented by the CLI.
    pub fn verify_id_token(&self, token: &str) -> Result<Identity> {
        let token: CoreIdToken = token.parse().context("malformed ID token")?;
        let audiences = &self.audiences;
        let verifier = CoreIdTokenVerifier::new_public_client(
            self.cli_client_id.clone(),
            self.issuer.clone(),
            self.jwks.clone(),
        )
        .set_other_audience_verifier_fn(|audience| {
            audiences.iter().any(|a| a == audience.as_str())
        });

        let claims = token
            .claims(&verifier, AnyNonce)
            .context("the ID token is not valid")?;
        Ok(self.identity(claims))
    }

    /// Maps the token claims to a login and a full name.
    fn identity(&self, claims: &CoreIdTokenClaims) -> Identity {
        let login = match self.login_claim {
            LoginClaim::PreferredUsername => claims.preferred_username().map(|u| u.to_string()),
            LoginClaim::Email => claims.email().map(|e| e.to_string()),
            LoginClaim::Subject => None,
        }
        .filter(|login| !login.is_empty())
        // Every provider has a subject, so it is the safe fallback.
        .unwrap_or_else(|| claims.subject().to_string());

        let name = claims
            .name()
            .and_then(|name| name.get(None))
            .map(|name| name.to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| login.clone());

        Identity { login, name }
    }
}

/// A student or teacher logged in in a browser.
///
/// Extracting it redirects to the OIDC provider when there is no valid session,
/// which is what makes every page of the web UI require authorization.
pub struct User(pub Identity);

impl FromRequestParts<AppState> for User {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let jar = SignedCookieJar::<Key>::from_request_parts(parts, state)
            .await
            .expect("extracting a cookie jar cannot fail");

        if let Some(cookie) = jar.get(SESSION_COOKIE) {
            if let Some(identity) = state.session(cookie.value()).await {
                return Ok(Self(identity));
            }
        }

        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("next", &return_path(&parts.method, parts.uri.path()))
            .finish();
        Err(Redirect::to(&format!("{}?{query}", views::link("/login"))).into_response())
    }
}

/// A user authenticated by an ID token in the `Authorization` header.
pub struct ApiUser(pub Identity);

impl FromRequestParts<AppState> for ApiUser {
    type Rejection = ApiFailure;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiFailure> {
        let header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| ApiFailure::unauthorized("missing Authorization header"))?;

        let token = header
            .strip_prefix("Bearer ")
            .or_else(|| header.strip_prefix("bearer "))
            .ok_or_else(|| ApiFailure::unauthorized("expected an Authorization: Bearer header"))?;

        let identity = state.oidc.verify_id_token(token.trim()).map_err(|error| {
            tracing::warn!(error = %format!("{error:#}"), "rejected an API token");
            ApiFailure::unauthorized("the ID token is not valid, run `quiz login`")
        })?;

        Ok(Self(identity))
    }
}

/// A teacher: an authenticated user that is allowed to ask questions.
pub struct Teacher(pub Identity);

impl FromRequestParts<AppState> for Teacher {
    type Rejection = ApiFailure;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiFailure> {
        let ApiUser(identity) = ApiUser::from_request_parts(parts, state).await?;
        if !state.cfg.is_teacher(&identity.login) {
            tracing::warn!(login = %identity.login, "refused a question from a non-teacher");
            return Err(ApiFailure::forbidden(format!(
                "{} is not allowed to ask questions",
                identity.login
            )));
        }
        Ok(Self(identity))
    }
}

#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    /// Where to send the browser after a successful login.
    next: Option<String>,
}

/// `GET /login` - redirects to the OIDC provider.
pub async fn login(State(state): State<AppState>, Query(query): Query<LoginQuery>) -> Response {
    let (url, csrf, nonce, pkce_verifier) = state.oidc.authorize_url();
    let next = sanitize_next(query.next.as_deref());

    let remembered = state
        .begin_login(
            csrf.secret().clone(),
            PendingLogin {
                pkce_verifier,
                nonce,
                next: next.clone(),
                created_at: Utc::now(),
            },
        )
        .await;

    if !remembered {
        tracing::warn!("too many logins in progress, refusing to start another");
        return PageError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Server je zaneprázdněn",
            "Právě probíhá příliš mnoho přihlášení. Zkuste to prosím za chvíli znovu.",
        )
        .into_response();
    }

    tracing::info!(%next, "redirecting a visitor to the OIDC provider");
    Redirect::to(url.as_str()).into_response()
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// `GET /auth/callback` - the provider redirects the browser here.
pub async fn callback(
    State(state): State<AppState>,
    Query(query): Query<CallbackQuery>,
    jar: SignedCookieJar,
) -> Result<Response, PageError> {
    if let Some(error) = query.error {
        let description = query.error_description.unwrap_or_default();
        tracing::warn!(%error, %description, "the OIDC provider reported an error");
        return Err(PageError::bad_request(format!(
            "Přihlášení se nezdařilo: {error} {description}"
        )));
    }

    let (Some(code), Some(csrf)) = (query.code, query.state) else {
        return Err(PageError::bad_request(
            "Odpověď od poskytovatele identity je neúplná.",
        ));
    };

    let pending = state.logins.write().await.remove(&csrf).ok_or_else(|| {
        tracing::warn!("no pending login for the returned state parameter");
        PageError::bad_request("Přihlášení vypršelo, zkuste to prosím znovu.")
    })?;

    let identity = state
        .oidc
        .exchange_code(code, pending.pkce_verifier, &pending.nonce)
        .await
        .map_err(|error| {
            tracing::warn!(error = %format!("{error:#}"), "login failed");
            PageError::new(
                axum::http::StatusCode::BAD_GATEWAY,
                "Přihlášení se nezdařilo",
                "Poskytovatel identity odmítl přihlášení.",
            )
        })?;

    let session_id = state.start_session(identity.clone()).await;
    let cookie = Cookie::build((SESSION_COOKIE, session_id))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(state.cfg.secure_cookies())
        .build();

    tracing::info!(
        login = %identity.login,
        name = %identity.name,
        "user logged in"
    );

    let target = views::link(&pending.next);
    Ok((jar.add(cookie), Redirect::to(&target)).into_response())
}

/// `GET /logout` - forgets the session.
pub async fn logout(State(state): State<AppState>, jar: SignedCookieJar) -> Response {
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        if let Some(identity) = state.session(cookie.value()).await {
            tracing::info!(login = %identity.login, "user logged out");
        }
        state.end_session(cookie.value()).await;
    }
    let jar = jar.remove(Cookie::from(SESSION_COOKIE));
    (jar, Redirect::to(&views::link("/"))).into_response()
}

/// Where to send the browser after the login it is about to be sent to.
///
/// A `POST` cannot be replayed after the login, and its path is usually not
/// even reachable with `GET`, so the enclosing page is used instead:
/// `POST /q/<id>/answer` comes back as `/q/<id>`.
fn return_path(method: &axum::http::Method, path: &str) -> String {
    if method == axum::http::Method::GET {
        return path.to_owned();
    }
    match path.trim_end_matches('/').rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => parent.to_owned(),
        _ => "/".to_owned(),
    }
}

/// Longest `next` parameter that is kept; the paths of this server are short.
const MAX_NEXT: usize = 256;

/// Keeps the `next` parameter from turning into an open redirect.
///
/// Only a path on this server is accepted. `//host` is the obvious way out,
/// but so is `/\host`: browsers normalize the backslash to a slash, so the
/// redirect would leave the site. Control characters are refused as well,
/// because the value ends up in a `Location` header.
fn sanitize_next(next: Option<&str>) -> String {
    let acceptable = |next: &&str| {
        next.len() <= MAX_NEXT
            && next.starts_with('/')
            && !next[1..].starts_with(['/', '\\'])
            && !next.contains('\\')
            && !next.chars().any(char::is_control)
    };
    match next.filter(acceptable) {
        Some(next) => next.to_owned(),
        None => "/".to_owned(),
    }
}

/// Fails the startup when the OIDC client is obviously unusable, or when
/// nobody, or everybody, would be allowed to ask questions.
pub fn check_config(cfg: &Config) -> Result<()> {
    if cfg.oidc_client_id.is_empty() {
        bail!("--oidc-client-id must not be empty");
    }
    cfg.check_teachers()
}

#[cfg(test)]
mod tests {
    use super::{return_path, sanitize_next};
    use axum::http::Method;

    #[test]
    fn a_post_comes_back_to_the_page_it_was_sent_from() {
        assert_eq!(return_path(&Method::GET, "/q/abc"), "/q/abc");
        assert_eq!(return_path(&Method::POST, "/q/abc/answer"), "/q/abc");
        assert_eq!(return_path(&Method::POST, "/join"), "/");
        assert_eq!(return_path(&Method::POST, "/"), "/");
    }

    #[test]
    fn next_stays_on_this_server() {
        assert_eq!(sanitize_next(Some("/q/abc")), "/q/abc");
        assert_eq!(sanitize_next(Some("//evil.example.com")), "/");
        assert_eq!(sanitize_next(Some("https://evil.example.com")), "/");
        assert_eq!(sanitize_next(None), "/");
        // A backslash is a slash to a browser, so `/\host` is `//host`.
        assert_eq!(sanitize_next(Some("/\\evil.example.com")), "/");
        assert_eq!(sanitize_next(Some("/q/\\\\evil.example.com")), "/");
        // Nothing that could break the Location header out of its line.
        assert_eq!(sanitize_next(Some("/q/a\r\nSet-Cookie: x=y")), "/");
        assert_eq!(sanitize_next(Some(&"/q/".repeat(200))), "/");
    }
}
