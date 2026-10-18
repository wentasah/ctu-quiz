//! Client for the quiz server's HTTP API.

use anyhow::{Context, Result, anyhow};
use quiz_common::api::{
    API_PREFIX, ApiError, CreateQuestion, OidcInfo, QuestionInfo, Results, RoomClosed, RoomStatus,
    WhoAmI,
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

    /// The room this teacher's students are in, and its questions.
    pub async fn room_status(&self) -> Result<RoomStatus> {
        self.send(self.get("room").await?).await
    }

    /// Asks the server to drop the room this teacher's students are in.
    pub async fn close_room(&self) -> Result<RoomClosed> {
        self.send(self.post("room/close").await?).await
    }

    pub async fn create_question(&self, question: &CreateQuestion) -> Result<QuestionInfo> {
        let request = self.post("questions").await?.json(question);
        self.send(request).await
    }

    /// Follows a question: the server sends its state right away and then
    /// again whenever it changes.
    pub async fn question_events(&self, id: &str) -> Result<Events> {
        let request = self.get(&format!("questions/{id}/events")).await?;
        let response = request
            .send()
            .await
            .with_context(|| format!("cannot reach the quiz server at {}", self.base))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(failure(status, &body));
        }
        Ok(Events {
            response,
            buffer: Vec::new(),
        })
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
    Err(failure(status, &body))
}

/// A useful error message for a response that is not a success.
fn failure(status: StatusCode, body: &str) -> anyhow::Error {
    let message = serde_json::from_str::<ApiError>(body)
        .map(|error| error.error)
        .unwrap_or_else(|_| body.trim().to_owned());

    match status {
        StatusCode::UNAUTHORIZED => anyhow!("not logged in: {message}"),
        StatusCode::FORBIDDEN => anyhow!("not allowed: {message}"),
        status => anyhow!("the server refused the request ({status}): {message}"),
    }
}

/// A stream of server-sent events.
pub struct Events {
    response: Response,
    /// What has arrived of the events not returned yet. Bytes rather than a
    /// string, because a chunk may end in the middle of a character.
    buffer: Vec<u8>,
}

/// One server-sent event.
#[derive(Debug, PartialEq, Eq)]
pub struct Event {
    pub name: String,
    pub data: String,
}

impl Events {
    /// The next event, or `None` once the server ends the stream.
    pub async fn next(&mut self) -> Result<Option<Event>> {
        loop {
            // Events are separated by an empty line.
            if let Some(end) = self.buffer.windows(2).position(|pair| pair == b"\n\n") {
                let block: Vec<u8> = self.buffer.drain(..end + 2).collect();
                // Keep-alive comments carry no event.
                if let Some(event) = parse_event(&String::from_utf8_lossy(&block)) {
                    return Ok(Some(event));
                }
                continue;
            }
            match self.response.chunk().await.context("the event stream broke off")? {
                Some(chunk) => self.buffer.extend_from_slice(&chunk),
                None => return Ok(None),
            }
        }
    }
}

/// Parses the lines of one event, or returns `None` when it has no data.
fn parse_event(block: &str) -> Option<Event> {
    let mut name = None;
    let mut data: Option<String> = None;
    for line in block.lines() {
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => name = Some(value.to_owned()),
            "data" => match &mut data {
                Some(data) => {
                    data.push('\n');
                    data.push_str(value);
                }
                None => data = Some(value.to_owned()),
            },
            // Comments, which start with a colon, and fields not used here.
            _ => {}
        }
    }
    Some(Event {
        name: name.unwrap_or_else(|| "message".to_owned()),
        data: data?,
    })
}

#[cfg(test)]
mod tests {
    use super::{Event, parse_event};

    #[test]
    fn server_sent_events_are_parsed() {
        assert_eq!(
            parse_event("event: state\ndata: {\"a\":1}\n\n"),
            Some(Event {
                name: "state".to_owned(),
                data: "{\"a\":1}".to_owned()
            })
        );
        assert_eq!(
            parse_event("data:one\ndata: two\n\n"),
            Some(Event {
                name: "message".to_owned(),
                data: "one\ntwo".to_owned()
            })
        );
        // A keep-alive comment.
        assert_eq!(parse_event(":\n\n"), None);
    }
}
