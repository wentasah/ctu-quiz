//! Logging the teacher in with OIDC.
//!
//! The CLI is a native, public OIDC client: it runs the authorization code flow
//! with PKCE against a loopback redirect URI (RFC 8252), which needs no client
//! secret. The resulting ID token is cached in the user's configuration
//! directory and sent to the quiz server in the `Authorization` header.

use std::{collections::HashMap, path::PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, TimeDelta, Utc};
use openidconnect::{
    AuthorizationCode, ClientId, CsrfToken, IssuerUrl, Nonce, OAuth2TokenResponse,
    PkceCodeChallenge, RedirectUrl, RefreshToken, Scope, TokenResponse,
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata},
};
use quiz_common::api::OidcInfo;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use url::Url;

/// An ID token that can still be used for a while.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub id_token: String,
    pub login: String,
    pub expires_at: DateTime<Utc>,
    /// Issuer and client the token belongs to; a changed server configuration
    /// must not silently reuse an old token.
    pub issuer: String,
    pub client_id: String,
    pub refresh_token: Option<String>,
}

impl Token {
    /// Tokens are refreshed a minute early, to survive a slow request.
    fn is_usable(&self, info: &OidcInfo) -> bool {
        self.issuer == info.issuer
            && self.client_id == info.client_id
            && self.expires_at > Utc::now() + TimeDelta::seconds(60)
    }
}

/// Cached tokens, one per quiz server.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    tokens: HashMap<String, Token>,
}

/// `$XDG_CONFIG_HOME/quiz/tokens.json`.
fn cache_path() -> Result<PathBuf> {
    let dir = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(
            std::env::var_os("HOME").context("neither XDG_CONFIG_HOME nor HOME is set")?,
        )
        .join(".config"),
    };
    Ok(dir.join("quiz").join("tokens.json"))
}

fn load_cache() -> Cache {
    let Ok(path) = cache_path() else {
        return Cache::default();
    };
    std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn store_cache(cache: &Cache) -> Result<()> {
    let path = cache_path()?;
    let dir = path.parent().context("the cache path has no directory")?;
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let json = serde_json::to_vec_pretty(cache)?;
    std::fs::write(&path, json).with_context(|| format!("cannot write {}", path.display()))?;

    // The file holds a bearer token, so keep it to ourselves.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("cannot chmod {}", path.display()))?;
    }
    Ok(())
}

/// Forgets the token for one server, or for all of them.
pub fn forget(server: Option<&str>) -> Result<bool> {
    let mut cache = load_cache();
    let removed = match server {
        Some(server) => cache.tokens.remove(server).is_some(),
        None => {
            let had_any = !cache.tokens.is_empty();
            cache.tokens.clear();
            had_any
        }
    };
    if removed {
        store_cache(&cache)?;
    }
    Ok(removed)
}

/// Returns a usable ID token for `server`, logging in if necessary.
pub async fn token(
    http: &reqwest::Client,
    server: &str,
    info: &OidcInfo,
    port: u16,
    open_browser: bool,
    force_login: bool,
) -> Result<Token> {
    let mut cache = load_cache();

    if !force_login {
        if let Some(token) = cache.tokens.get(server) {
            if token.is_usable(info) {
                return Ok(token.clone());
            }
            // An expired token may still be refreshable without a browser.
            if token.issuer == info.issuer && token.client_id == info.client_id {
                if let Some(refresh) = token.refresh_token.clone() {
                    match refresh_token(http, info, &refresh).await {
                        Ok(token) => {
                            cache.tokens.insert(server.to_owned(), token.clone());
                            store_cache(&cache)?;
                            return Ok(token);
                        }
                        Err(error) => {
                            eprintln!("Refreshing the login failed ({error:#}), logging in again.");
                        }
                    }
                }
            }
        }
    }

    let token = interactive_login(http, info, port, open_browser).await?;
    cache.tokens.insert(server.to_owned(), token.clone());
    store_cache(&cache)?;
    Ok(token)
}

/// Discovers the provider and builds a client for the loopback redirect URI.
async fn discover(
    http: &reqwest::Client,
    info: &OidcInfo,
    redirect_uri: &str,
) -> Result<
    CoreClient<
        openidconnect::EndpointSet,
        openidconnect::EndpointNotSet,
        openidconnect::EndpointNotSet,
        openidconnect::EndpointNotSet,
        openidconnect::EndpointMaybeSet,
        openidconnect::EndpointMaybeSet,
    >,
> {
    let issuer = IssuerUrl::new(info.issuer.clone()).context("the server reported a bad issuer")?;
    let metadata = CoreProviderMetadata::discover_async(issuer, http)
        .await
        .with_context(|| format!("OIDC discovery at {} failed", info.issuer))?;

    Ok(
        CoreClient::from_provider_metadata(metadata, ClientId::new(info.client_id.clone()), None)
            .set_redirect_uri(RedirectUrl::new(redirect_uri.to_owned())?),
    )
}

/// Opens the browser and waits for the provider to redirect back.
async fn interactive_login(
    http: &reqwest::Client,
    info: &OidcInfo,
    port: u16,
    open_browser: bool,
) -> Result<Token> {
    // Bind first: with `--login-port 0` the port is only known afterwards.
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("cannot listen on 127.0.0.1:{port} for the OIDC redirect"))?;
    let port = listener.local_addr()?.port();

    let redirect_uri = match &info.redirect_uri {
        Some(uri) => uri.clone(),
        None => format!("http://127.0.0.1:{port}/callback"),
    };
    let client = discover(http, info, &redirect_uri).await?;

    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let mut request = client.authorize_url(
        CoreAuthenticationFlow::AuthorizationCode,
        CsrfToken::new_random,
        Nonce::new_random,
    );
    for scope in info.scopes.iter().filter(|scope| *scope != "openid") {
        request = request.add_scope(Scope::new(scope.clone()));
    }
    let (auth_url, csrf, nonce) = request.set_pkce_challenge(challenge).url();

    println!("Logging in at {}", info.issuer);
    if !open_browser || open::that_detached(auth_url.as_str()).is_err() {
        println!("Open this URL in a browser to log in:\n\n    {auth_url}\n");
    } else {
        println!("A browser window was opened for the login.");
        println!("If you do not see it, open this URL:\n\n    {auth_url}\n");
    }

    let (code, state) = wait_for_redirect(&listener).await?;
    if state != *csrf.secret() {
        bail!("the state parameter returned by the provider does not match");
    }

    let response = client
        .exchange_code(AuthorizationCode::new(code))
        .context("the provider has no token endpoint")?
        .set_pkce_verifier(verifier)
        .request_async(http)
        .await
        .context("exchanging the authorization code failed")?;

    let id_token = response
        .id_token()
        .ok_or_else(|| anyhow!("the provider returned no ID token"))?;
    let claims = id_token
        .claims(&client.id_token_verifier(), &nonce)
        .context("the ID token is not valid")?;

    Ok(Token {
        id_token: id_token.to_string(),
        login: claims
            .preferred_username()
            .map(|name| name.to_string())
            .unwrap_or_else(|| claims.subject().to_string()),
        expires_at: claims.expiration(),
        issuer: info.issuer.clone(),
        client_id: info.client_id.clone(),
        refresh_token: response.refresh_token().map(|token| token.secret().clone()),
    })
}

/// Uses a refresh token to get a new ID token without opening a browser.
async fn refresh_token(http: &reqwest::Client, info: &OidcInfo, refresh: &str) -> Result<Token> {
    // The redirect URI is not used by the refresh grant, but the client needs
    // a valid one.
    let client = discover(http, info, "http://127.0.0.1:1/callback").await?;
    let response = client
        .exchange_refresh_token(&RefreshToken::new(refresh.to_owned()))
        .context("the provider has no token endpoint")?
        .request_async(http)
        .await
        .context("refreshing the token failed")?;

    let id_token = response
        .id_token()
        .ok_or_else(|| anyhow!("the refreshed response contains no ID token"))?;
    let claims = id_token
        .claims(&client.id_token_verifier(), no_nonce)
        .context("the refreshed ID token is not valid")?;

    Ok(Token {
        id_token: id_token.to_string(),
        login: claims
            .preferred_username()
            .map(|name| name.to_string())
            .unwrap_or_else(|| claims.subject().to_string()),
        expires_at: claims.expiration(),
        issuer: info.issuer.clone(),
        client_id: info.client_id.clone(),
        refresh_token: response
            .refresh_token()
            .map(|token| token.secret().clone())
            .or_else(|| Some(refresh.to_owned())),
    })
}

/// A refreshed ID token may legitimately carry no nonce.
fn no_nonce(_nonce: Option<&Nonce>) -> Result<(), String> {
    Ok(())
}

/// Serves the loopback redirect URI until the provider sends the code.
async fn wait_for_redirect(listener: &TcpListener) -> Result<(String, String)> {
    loop {
        let (mut socket, _) = listener
            .accept()
            .await
            .context("waiting for the OIDC redirect failed")?;

        let mut buffer = [0u8; 8192];
        let read = socket.read(&mut buffer).await?;
        let request = String::from_utf8_lossy(&buffer[..read]);
        let target = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/");

        // A relative target only becomes a URL with some base.
        let url = Url::parse("http://127.0.0.1/")
            .and_then(|base| base.join(target))
            .context("the browser sent a malformed request")?;
        let params: HashMap<_, _> = url.query_pairs().into_owned().collect();

        if let Some(error) = params.get("error") {
            let description = params
                .get("error_description")
                .map(String::as_str)
                .unwrap_or("");
            reply(&mut socket, "Login failed", "You can close this window.").await;
            bail!("the provider refused the login: {error} {description}");
        }

        match (params.get("code"), params.get("state")) {
            (Some(code), Some(state)) => {
                reply(
                    &mut socket,
                    "Logged in",
                    "You can close this window and go back to the terminal.",
                )
                .await;
                return Ok((code.clone(), state.clone()));
            }
            // Ignore anything else the browser asks for, e.g. /favicon.ico.
            _ => reply(&mut socket, "Waiting for the login", "Nothing to see here.").await,
        }
    }
}

async fn reply(socket: &mut TcpStream, title: &str, message: &str) {
    let body = format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <title>{title}</title></head>\
         <body style=\"font: 16px system-ui; margin: 4rem auto; max-width: 30rem\">\
         <h1>{title}</h1><p>{message}</p></body></html>"
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.flush().await;
}
