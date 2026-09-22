//! Client for the quiz server's HTTP API.

use anyhow::{Context, Result, bail};
use quiz_common::api::{
    API_PREFIX, ApiError, CreateQuestion, OidcInfo, QuestionInfo, Results, WhoAmI,
};
use reqwest::{RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use url::Url;

/// An authenticated connection to one quiz server.
pub struct Api {
    http: reqwest::Client,
    base: Url,
    id_token: String,
}

impl Api {
    pub fn new(http: reqwest::Client, base: Url, id_token: String) -> Self {
        Self {
            http,
            base,
            id_token,
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
        self.send(self.get("whoami")?).await
    }

    pub async fn create_question(&self, question: &CreateQuestion) -> Result<QuestionInfo> {
        let request = self.post("questions")?.json(question);
        self.send(request).await
    }

    pub async fn question(&self, id: &str) -> Result<QuestionInfo> {
        self.send(self.get(&format!("questions/{id}"))?).await
    }

    pub async fn close(&self, id: &str) -> Result<QuestionInfo> {
        self.send(self.post(&format!("questions/{id}/close"))?)
            .await
    }

    pub async fn results(&self, id: &str) -> Result<Results> {
        self.send(self.get(&format!("questions/{id}/results"))?)
            .await
    }

    fn get(&self, path: &str) -> Result<RequestBuilder> {
        Ok(self
            .http
            .get(endpoint(&self.base, path)?)
            .bearer_auth(&self.id_token))
    }

    fn post(&self, path: &str) -> Result<RequestBuilder> {
        Ok(self
            .http
            .post(endpoint(&self.base, path)?)
            .bearer_auth(&self.id_token))
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
