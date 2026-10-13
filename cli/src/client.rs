//! Client for the quiz server's HTTP API.

use anyhow::{Context, Result, bail};
use quiz_common::api::{
    API_PREFIX, ApiError, CreateQuestion, OidcInfo, QuestionInfo, Results, RoomClosed, WhoAmI,
};
use reqwest::{RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use tokio::sync::Mutex;
use url::Url;

use crate::auth::{self, Token};

/// How to log in again once the access token runs out.
pub struct Login {
    pub info: OidcInfo,
    pub port: u16,
    pub open_browser: bool,
}

/// An authenticated connection to one quiz server.
pub struct Api {
    http: reqwest::Client,
    base: Url,
    token: Mutex<Token>,
    /// Without it, the token is used as it is, even when it has expired.
    login: Option<Login>,
}

impl Api {
    pub fn new(http: reqwest::Client, base: Url, token: Token, login: Option<Login>) -> Self {
        Self {
            http,
            base,
            token: Mutex::new(token),
            login,
        }
    }

    /// Asks the server how to log in. This is the only endpoint that needs no
    /// token, which is what lets the CLI be configured with just a server URL.
    pub async fn oidc_info(http: &reqwest::Client, base: &Url) -> Result<OidcInfo> {
        let url = endpoint(base, "oidc")?;
        let response = http
            .get(url.clone())
            .send()
            .await
            .with_context(|| format!("cannot reach the quiz server at {url}"))?;
        parse(response).await
    }

    pub async fn whoami(&self) -> Result<WhoAmI> {
        self.send(self.get("whoami").await?).await
    }

    /// Asks the server to drop the room this teacher's students are in.
    pub async fn close_room(&self) -> Result<RoomClosed> {
        self.send(self.post("room/close").await?).await
    }

    pub async fn create_question(&self, question: &CreateQuestion) -> Result<QuestionInfo> {
        let request = self.post("questions").await?.json(question);
        self.send(request).await
    }

    pub async fn question(&self, id: &str) -> Result<QuestionInfo> {
        self.send(self.get(&format!("questions/{id}")).await?).await
    }

    pub async fn close(&self, id: &str) -> Result<QuestionInfo> {
        self.send(self.post(&format!("questions/{id}/close")).await?)
            .await
    }

    pub async fn reveal(&self, id: &str) -> Result<QuestionInfo> {
        self.send(self.post(&format!("questions/{id}/reveal")).await?)
            .await
    }

    pub async fn results(&self, id: &str) -> Result<Results> {
        self.send(self.get(&format!("questions/{id}/results")).await?)
            .await
    }

    async fn get(&self, path: &str) -> Result<RequestBuilder> {
        let token = self.access_token().await?;
        Ok(self
            .http
            .get(endpoint(&self.base, path)?)
            .bearer_auth(token))
    }

    async fn post(&self, path: &str) -> Result<RequestBuilder> {
        let token = self.access_token().await?;
        Ok(self
            .http
            .post(endpoint(&self.base, path)?)
            .bearer_auth(token))
    }

    /// The access token, refreshed first when it is about to expire.
    ///
    /// Access tokens may live only a few minutes, less than a question stays
    /// open, so `quiz ask` would otherwise lose its login half way through.
    async fn access_token(&self) -> Result<String> {
        let mut token = self.token.lock().await;
        if let Some(login) = &self.login {
            if !token.is_usable(&login.info) {
                *token = auth::token(
                    &self.http,
                    self.base.as_str(),
                    &login.info,
                    login.port,
                    login.open_browser,
                    false,
                )
                .await?;
            }
        }
        Ok(token.access_token.clone())
    }

    async fn send<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T> {
        let response = request
            .send()
            .await
            .with_context(|| format!("cannot reach the quiz server at {}", self.base))?;
        parse(response).await
    }
}

/// Builds the URL of an API endpoint, honouring a path prefix in `base`.
fn endpoint(base: &Url, path: &str) -> Result<Url> {
    let mut base = base.clone();
    if !base.path().ends_with('/') {
        let path = format!("{}/", base.path());
        base.set_path(&path);
    }
    base.join(&format!("{}/{path}", API_PREFIX.trim_start_matches('/')))
        .with_context(|| format!("cannot build an API URL from {base}"))
}

/// Turns a response into the expected type, or into a useful error message.
async fn parse<T: DeserializeOwned>(response: Response) -> Result<T> {
    let status = response.status();
    let body = response
        .text()
        .await
        .context("reading the response failed")?;

    if status.is_success() {
        return serde_json::from_str(&body)
            .with_context(|| format!("the server sent an unexpected response: {body}"));
    }

    let message = serde_json::from_str::<ApiError>(&body)
        .map(|error| error.error)
        .unwrap_or_else(|_| body.trim().to_owned());

    match status {
        StatusCode::UNAUTHORIZED => bail!("not logged in: {message}"),
        StatusCode::FORBIDDEN => bail!("not allowed: {message}"),
        status => bail!("the server refused the request ({status}): {message}"),
    }
}
