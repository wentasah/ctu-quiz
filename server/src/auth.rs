//! Authentication and authorization, all of it based on OIDC.
//!
//! Students log in in the browser with the authorization code flow and get a
//! signed session cookie. Teachers log in with the CLI, which performs the same
//! flow on a loopback address and then sends the resulting access token in the
//! `Authorization: Bearer` header; [`Oidc::verify_access_token`] validates it
//! against the provider's signing keys.
//!
//! The ID token stays with the CLI: it tells the CLI who logged in, and it is
//! addressed to the CLI. The access token is what the provider issues for
//! calling an API, and its audience says which API.

use anyhow::{Context, Result, anyhow, bail, ensure};
use axum::{
    extract::{FromRequestParts, Query, State},
    http::{header, request::Parts},
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::{Cookie, Key, SameSite, SignedCookieJar};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, IssuerUrl, JsonWebKey, JsonWebKeyAlgorithm, JsonWebKeyId, JsonWebKeyUse,
    JwsSigningAlgorithm, Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
    TokenResponse,
    core::{
        CoreAuthenticationFlow, CoreClient, CoreJsonWebKeySet, CoreJwsSigningAlgorithm,
        CoreProviderMetadata,
    },
};
use serde::Deserialize;
use url::Url;

use crate::{
    config::{Config, LoginClaim},
    error::{ApiFailure, PageError},
    state::{AppState, LOGIN_TTL_MINUTES, PendingLogin},
    views,
};

/// Name of the cookie holding the (signed) session id.
pub const SESSION_COOKIE: &str = "quiz_session";

/// Name of the cookie that ties a login to the browser which started it.
///
/// The `state` of the login is part of the name, so that logins started in
/// several tabs at once do not overwrite each other's cookie. Over HTTPS the
/// name gets the `__Host-` prefix, with which the browser refuses the cookie
/// unless this very host set it: a page on a sibling subdomain cannot plant
/// a login cookie it got for itself.
fn login_cookie_name(csrf: &str, secure: bool) -> String {
    let prefix = if secure { "__Host-" } else { "" };
    format!("{prefix}quiz_login_{csrf}")
}

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
    /// Roles from the `--roles-claim` of an access token. Empty for a browser
    /// session, where the server does not need them.
    pub roles: Vec<String>,
}

/// The JOSE header of a signed JWT.
#[derive(Debug, Deserialize)]
struct JwtHeader {
    alg: CoreJwsSigningAlgorithm,
    kid: Option<JsonWebKeyId>,
}

/// The claims of an access token that the server looks at.
///
/// Access tokens have no standard format; this is what Keycloak puts in them,
/// which matches RFC 9068 plus the user's profile claims.
#[derive(Debug, Deserialize)]
struct AccessTokenClaims {
    iss: String,
    #[serde(default)]
    aud: Audiences,
    exp: i64,
    nbf: Option<i64>,
    /// Keycloak tells its tokens apart by `typ`: `Bearer` for an access
    /// token, `ID` for an ID token.
    typ: Option<String>,
    sub: Option<String>,
    preferred_username: Option<String>,
    email: Option<String>,
    name: Option<String>,
}

/// The `aud` claim: a single string or an array of them.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Audiences {
    One(String),
    Many(Vec<String>),
}

impl Default for Audiences {
    fn default() -> Self {
        Self::Many(Vec::new())
    }
}

impl Audiences {
    fn as_slice(&self) -> &[String] {
        match self {
            Self::One(one) => std::slice::from_ref(one),
            Self::Many(many) => many,
        }
    }
}

/// Everything needed to talk to the OIDC provider.
pub struct Oidc {
    client: DiscoveredClient,
    http: reqwest::Client,
    issuer: IssuerUrl,
    jwks: CoreJsonWebKeySet,
    /// Audiences that make an access token one for this server: its client
    /// ID and whatever `--oidc-audiences` adds.
    audiences: Vec<String>,
    scopes: Vec<Scope>,
    login_claim: LoginClaim,
    roles_claim: String,
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
            audiences: std::iter::once(cfg.oidc_client_id.clone())
                .chain(cfg.oidc_audiences.iter().cloned())
                .collect(),
            // "openid" is always sent by the crate itself.
            scopes: cfg
                .oidc_scopes
                .iter()
                .filter(|s| *s != "openid")
                .map(|s| Scope::new(s.clone()))
                .collect(),
            login_claim: cfg.login_claim,
            roles_claim: cfg.roles_claim.clone(),
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

        Ok(self.identity(
            Some(claims.subject().as_str()),
            claims.preferred_username().map(|u| u.as_str()),
            claims.email().map(|e| e.as_str()),
            claims
                .name()
                .and_then(|name| name.get(None))
                .map(|n| n.as_str()),
        ))
    }

    /// Validates an access token presented by the CLI.
    ///
    /// The token has to be a JWT signed by the provider, still valid, and
    /// issued for this server: its audience must contain the server's client
    /// ID, whichever client (the CLI's own) asked for it.
    pub fn verify_access_token(&self, token: &str) -> Result<Identity> {
        let mut parts = token.split('.');
        let (Some(header), Some(payload), Some(signature), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            bail!("the access token is not a signed JWT");
        };
        let decode = |part: &str| {
            URL_SAFE_NO_PAD
                .decode(part)
                .context("bad base64 in the JWT")
        };

        let header: JwtHeader =
            serde_json::from_slice(&decode(header)?).context("malformed JWT header")?;
        // An HMAC key would be a secret shared with the provider, which this
        // server does not have; anybody could forge such a token.
        ensure!(
            !header.alg.uses_shared_secret() && header.alg.key_type().is_some(),
            "the access token is not signed with a public key"
        );

        // The `kid` and `alg` of the header are chosen by whoever made the
        // token, so they only select among the provider's own keys.
        let message = &token.as_bytes()[..token.len() - signature.len() - 1];
        let signature = decode(signature)?;
        let verified = self
            .jwks
            .keys()
            .iter()
            .filter(|key| header.kid.is_none() || key.key_id() == header.kid.as_ref())
            .filter(|key| {
                key.key_use()
                    .is_none_or(|key_use| key_use.allows_signature())
            })
            .filter(|key| header.alg.key_type().as_ref() == Some(key.key_type()))
            .filter(|key| match key.signing_alg() {
                JsonWebKeyAlgorithm::Unspecified => true,
                JsonWebKeyAlgorithm::Algorithm(alg) => *alg == header.alg,
                JsonWebKeyAlgorithm::Unsupported => false,
            })
            .any(|key| {
                key.verify_signature(&header.alg, message, &signature)
                    .is_ok()
            });
        ensure!(verified, "the access token is not signed by the provider");

        let payload = decode(payload)?;
        let claims: AccessTokenClaims =
            serde_json::from_slice(&payload).context("malformed access token claims")?;
        ensure!(
            claims.iss == self.issuer.as_str(),
            "the access token was issued by {}, not by {}",
            claims.iss,
            self.issuer.as_str()
        );
        ensure!(
            self.audiences
                .iter()
                .any(|a| claims.aud.as_slice().contains(a)),
            "the access token is not meant for this server: its audience is {:?}, not one of {:?}",
            claims.aud.as_slice(),
            self.audiences
        );
        let now = Utc::now().timestamp();
        ensure!(claims.exp > now, "the access token has expired");
        ensure!(
            claims.nbf.is_none_or(|nbf| nbf <= now),
            "the access token is not valid yet"
        );
        if let Some(typ) = &claims.typ {
            ensure!(
                typ.eq_ignore_ascii_case("Bearer"),
                "the token is not an access token but `{typ}`"
            );
        }

        let mut identity = self.identity(
            claims.sub.as_deref(),
            claims.preferred_username.as_deref(),
            claims.email.as_deref(),
            claims.name.as_deref(),
        );
        identity.roles = self.roles(&payload)?;
        Ok(identity)
    }

    /// The roles listed in the `--roles-claim` of a token's payload: a single
    /// string or an array of them. A missing claim means no roles.
    fn roles(&self, payload: &[u8]) -> Result<Vec<String>> {
        let mut claims: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(payload).context("malformed access token claims")?;
        let Some(roles) = claims.remove(&self.roles_claim) else {
            return Ok(Vec::new());
        };
        let roles = match roles {
            serde_json::Value::String(role) => vec![role],
            roles => serde_json::from_value(roles).with_context(|| {
                format!(
                    "the `{}` claim is neither a string nor an array of strings",
                    self.roles_claim
                )
            })?,
        };
        Ok(roles)
    }

    /// Maps the token claims to a login and a full name.
    fn identity(
        &self,
        subject: Option<&str>,
        preferred_username: Option<&str>,
        email: Option<&str>,
        name: Option<&str>,
    ) -> Identity {
        fn non_empty(value: Option<&str>) -> Option<&str> {
            value.filter(|value| !value.is_empty())
        }
        let login = match self.login_claim {
            LoginClaim::PreferredUsername => non_empty(preferred_username),
            LoginClaim::Email => non_empty(email),
            LoginClaim::Subject => None,
        }
        // Every provider has a subject, so it is the safe fallback.
        .or(non_empty(subject))
        .unwrap_or_default()
        .to_owned();

        let name = non_empty(name).unwrap_or(&login).to_owned();
        Identity {
            login,
            name,
            roles: Vec::new(),
        }
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

/// A user authenticated by an access token in the `Authorization` header.
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

        let identity = state
            .oidc
            .verify_access_token(token.trim())
            .map_err(|error| {
                tracing::warn!(error = %format!("{error:#}"), "rejected an API token");
                ApiFailure::unauthorized("the access token is not valid, run `quiz login`")
            })?;
        // Without a login there is nobody to check against the teacher list.
        if identity.login.is_empty() {
            tracing::warn!("rejected an access token without a subject");
            return Err(ApiFailure::unauthorized(
                "the access token does not say who you are",
            ));
        }
        if state.cfg.is_teacher(&identity) {
            state.metrics.teacher_seen(&identity.login);
        }

        Ok(Self(identity))
    }
}

/// A teacher: an authenticated user that is allowed to ask questions.
pub struct Teacher(pub Identity);

impl FromRequestParts<AppState> for Teacher {
    type Rejection = ApiFailure;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiFailure> {
        let ApiUser(identity) = ApiUser::from_request_parts(parts, state).await?;
        if !state.cfg.is_teacher(&identity) {
            tracing::warn!(
                login = %identity.login,
                roles = ?identity.roles,
                "refused a question from a non-teacher"
            );
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
///
/// The browser also gets a cookie that the callback insists on. Without it,
/// anybody could start a login, sign in as themselves, and hand the callback
/// URL to somebody else, whose browser would then finish the login and be
/// logged in as the attacker: the victim's answers would be recorded under
/// the attacker's name.
pub async fn login(
    State(state): State<AppState>,
    Query(query): Query<LoginQuery>,
    jar: SignedCookieJar,
) -> Response {
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
            "server-busy",
            "too-many-logins",
        )
        .into_response();
    }

    let secure = state.cfg.secure_cookies();
    let cookie = Cookie::build((login_cookie_name(csrf.secret(), secure), "1"))
        .path("/")
        .http_only(true)
        // The provider sends the browser back with a top-level GET, which
        // carries a `Lax` cookie even though it comes from another site.
        .same_site(SameSite::Lax)
        .secure(secure)
        .max_age(time::Duration::minutes(LOGIN_TTL_MINUTES))
        .build();

    tracing::info!(%next, "redirecting a visitor to the OIDC provider");
    (jar.add(cookie), Redirect::to(url.as_str())).into_response()
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
        return Err(PageError::bad_request("login-error")
            .arg("error", error)
            .arg("description", description));
    }

    let (Some(code), Some(csrf)) = (query.code, query.state) else {
        return Err(PageError::bad_request("login-incomplete"));
    };

    // Only the browser that started the login may finish it.
    let login_cookie = login_cookie_name(&csrf, state.cfg.secure_cookies());
    if jar.get(&login_cookie).is_none() {
        tracing::warn!("a login was finished by a browser that did not start it");
        return Err(PageError::bad_request("login-other-browser"));
    }

    let pending = state.logins.write().await.remove(&csrf).ok_or_else(|| {
        tracing::warn!("no pending login for the returned state parameter");
        PageError::bad_request("login-expired")
    })?;

    let identity = state
        .oidc
        .exchange_code(code, pending.pkce_verifier, &pending.nonce)
        .await
        .map_err(|error| {
            tracing::warn!(error = %format!("{error:#}"), "login failed");
            PageError::new(
                axum::http::StatusCode::BAD_GATEWAY,
                "login-failed",
                "login-refused",
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
    let jar = jar
        .add(cookie)
        .remove(Cookie::build(login_cookie).path("/"));
    Ok((jar, Redirect::to(&target)).into_response())
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
    if cfg.roles_claim.is_empty() {
        bail!("--roles-claim must not be empty");
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
