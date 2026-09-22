//! Command line and environment configuration of the server.

use std::{net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use url::Url;

/// Which OIDC claim identifies a user. On the CTU/FEE identity provider
/// `preferred_username` is the familiar eight-character login (`sojkam1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum LoginClaim {
    PreferredUsername,
    Email,
    Subject,
}

/// Configuration of the quiz server.
///
/// Every option can also be given as an environment variable, which is what
/// the systemd unit and the container image use.
#[derive(Debug, Clone, Parser)]
#[command(version, about, long_about = None)]
pub struct Config {
    /// Address and port to listen on.
    #[arg(long, env = "QUIZ_LISTEN", default_value = "127.0.0.1:8080")]
    pub listen: SocketAddr,

    /// Public base URL of the server, as seen by students.
    ///
    /// Used to build the URL in the QR code and the OIDC redirect URI, so it
    /// has to match what the reverse proxy serves.
    #[arg(long, env = "QUIZ_PUBLIC_URL", default_value = "http://localhost:8080")]
    pub public_url: Url,

    /// File all operations are logged to.
    #[arg(long, env = "QUIZ_LOG_FILE", default_value = "quiz-server.log")]
    pub log_file: PathBuf,

    /// Log level or `tracing` filter directives. `RUST_LOG` takes precedence.
    #[arg(long, env = "QUIZ_LOG_FILTER", default_value = "info")]
    pub log_filter: String,

    /// URL of the OIDC provider, without `/.well-known/openid-configuration`.
    #[arg(long, env = "QUIZ_OIDC_ISSUER")]
    pub oidc_issuer: Url,

    /// OIDC client ID of the web server.
    #[arg(long, env = "QUIZ_OIDC_CLIENT_ID")]
    pub oidc_client_id: String,

    /// OIDC client secret. Leave unset for a public client.
    #[arg(long, env = "QUIZ_OIDC_CLIENT_SECRET", hide_env_values = true)]
    pub oidc_client_secret: Option<String>,

    /// Client ID the teacher CLI should use. Defaults to `--oidc-client-id`.
    #[arg(long, env = "QUIZ_OIDC_CLI_CLIENT_ID")]
    pub oidc_cli_client_id: Option<String>,

    /// Redirect URI registered for the teacher CLI. Leave unset if the
    /// provider accepts any loopback port (RFC 8252).
    #[arg(long, env = "QUIZ_OIDC_CLI_REDIRECT_URI")]
    pub oidc_cli_redirect_uri: Option<Url>,

    /// Scopes requested during login.
    #[arg(
        long,
        env = "QUIZ_OIDC_SCOPES",
        value_delimiter = ',',
        default_value = "openid,profile,email"
    )]
    pub oidc_scopes: Vec<String>,

    /// Further audiences accepted in ID tokens presented by the CLI.
    #[arg(long, env = "QUIZ_OIDC_AUDIENCES", value_delimiter = ',')]
    pub oidc_audiences: Vec<String>,

    /// Logins allowed to ask questions. Empty means every user that can log in.
    #[arg(long, env = "QUIZ_TEACHERS", value_delimiter = ',')]
    pub teachers: Vec<String>,

    /// Claim used as the login name of a user.
    #[arg(long, env = "QUIZ_LOGIN_CLAIM", default_value = "preferred-username")]
    pub login_claim: LoginClaim,

    /// How long a question is kept in memory after it was created.
    #[arg(long, env = "QUIZ_QUESTION_TTL_HOURS", default_value_t = 12)]
    pub question_ttl_hours: i64,

    /// How long a browser session stays valid.
    #[arg(long, env = "QUIZ_SESSION_TTL_HOURS", default_value_t = 12)]
    pub session_ttl_hours: i64,

    /// Take the client address from `X-Forwarded-For` when logging.
    ///
    /// Only enable this behind a reverse proxy that sets the header itself.
    #[arg(long, env = "QUIZ_TRUST_PROXY")]
    pub trust_proxy: bool,
}

impl Config {
    /// Normalizes the configuration: an empty `QUIZ_TEACHERS=` must not turn
    /// into a list with one empty login, and the public URL has to end with a
    /// slash so that relative URLs can be joined onto it.
    pub fn normalize(mut self) -> Result<Self> {
        self.teachers = clean(self.teachers);
        self.oidc_audiences = clean(self.oidc_audiences);
        self.oidc_scopes = clean(self.oidc_scopes);

        if !self.public_url.path().ends_with('/') {
            let path = format!("{}/", self.public_url.path());
            self.public_url.set_path(&path);
        }
        // Fail early rather than when the first QR code is generated.
        self.url("q/test")
            .context("--public-url cannot be used as a base URL")?;
        Ok(self)
    }

    /// Builds an absolute URL from a path relative to the public base URL.
    pub fn url(&self, relative: &str) -> Result<Url> {
        Ok(self.public_url.join(relative)?)
    }

    /// Path the server is served under, without a trailing slash.
    ///
    /// Empty when the server owns the whole host, `/quiz` when it is behind a
    /// reverse proxy that forwards `https://host/quiz/...` unchanged.
    pub fn prefix(&self) -> String {
        self.public_url.path().trim_end_matches('/').to_owned()
    }

    /// Where the OIDC provider sends the student back after login.
    pub fn redirect_uri(&self) -> Result<Url> {
        self.url("auth/callback")
    }

    /// Client ID for the teacher CLI.
    pub fn cli_client_id(&self) -> &str {
        self.oidc_cli_client_id
            .as_deref()
            .unwrap_or(&self.oidc_client_id)
    }

    /// Whether session cookies may be restricted to HTTPS.
    pub fn secure_cookies(&self) -> bool {
        self.public_url.scheme() == "https"
    }

    /// Whether `login` is allowed to ask questions.
    ///
    /// An empty teacher list means everybody who can log in may ask.
    pub fn is_teacher(&self, login: &str) -> bool {
        self.teachers.is_empty() || self.teachers.iter().any(|t| t == login)
    }
}

/// Drops empty entries from a comma-separated list option.
fn clean(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::Config;
    use clap::Parser;

    fn config(args: &[&str]) -> Config {
        let mut argv = vec![
            "quiz-server",
            "--oidc-issuer",
            "https://id.example.org",
            "--oidc-client-id",
            "quiz",
        ];
        argv.extend_from_slice(args);
        Config::parse_from(argv).normalize().unwrap()
    }

    #[test]
    fn an_empty_teacher_list_allows_everybody() {
        let cfg = config(&["--teachers", ""]);
        assert!(cfg.teachers.is_empty());
        assert!(cfg.is_teacher("anybody"));
    }

    #[test]
    fn a_teacher_list_is_split_and_trimmed() {
        let cfg = config(&["--teachers", "sojkam1, vlasakji"]);
        assert_eq!(cfg.teachers, ["sojkam1", "vlasakji"]);
        assert!(cfg.is_teacher("vlasakji"));
        assert!(!cfg.is_teacher("novakja1"));
    }

    #[test]
    fn urls_are_built_from_the_public_url() {
        let cfg = config(&["--public-url", "https://osy.fel.cvut.cz/quiz"]);
        assert_eq!(cfg.public_url.as_str(), "https://osy.fel.cvut.cz/quiz/");
        assert_eq!(cfg.prefix(), "/quiz");
        assert_eq!(
            cfg.redirect_uri().unwrap().as_str(),
            "https://osy.fel.cvut.cz/quiz/auth/callback"
        );
        assert_eq!(
            cfg.url("q/abc").unwrap().as_str(),
            "https://osy.fel.cvut.cz/quiz/q/abc"
        );
        assert!(cfg.secure_cookies());

        let cfg = config(&["--public-url", "http://localhost:8080"]);
        assert_eq!(cfg.prefix(), "");
        assert!(!cfg.secure_cookies());
    }

    #[test]
    fn the_cli_client_defaults_to_the_web_client() {
        assert_eq!(config(&[]).cli_client_id(), "quiz");
        assert_eq!(
            config(&["--oidc-cli-client-id", "quiz-cli"]).cli_client_id(),
            "quiz-cli"
        );
    }
}
