//! The quiz server: shows questions to students, collects their answers and
//! serves the API used by the teacher CLI.
//!
//! All state lives in memory (see [`state`]); the only file the server writes
//! is the log file, and it never reads it back. Unless `--state-file` is
//! given: then the rooms, questions and answers are saved there on shutdown
//! and read back on start (see [`persist`]).

pub mod api;
pub mod auth;
pub mod config;
pub mod display;
pub mod error;
pub mod i18n;
pub mod live;
pub mod metrics;
pub mod persist;
pub mod state;
pub mod views;
pub mod web;

use std::time::Duration;

use anyhow::{Context, Result};
use axum::{
    Router,
    body::Body,
    extract::DefaultBodyLimit,
    http::{Request, header},
    middleware,
    response::IntoResponse,
    routing::{get, post},
};
use quiz_common::api::API_PREFIX;
use tokio::net::TcpListener;
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::Level;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{auth::Oidc, config::Config, state::AppState};

/// How often expired sessions and old questions are dropped from memory.
const PRUNE_INTERVAL: Duration = Duration::from_secs(300);

/// Sets up logging into the log file and onto the standard error output
/// (requirement 5).
///
/// The returned guard flushes the log file when it is dropped, so it has to be
/// kept alive for as long as the server runs.
pub fn init_logging(cfg: &Config) -> Result<WorkerGuard> {
    let directory = cfg
        .log_file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let file_name = cfg
        .log_file
        .file_name()
        .context("--log-file must end with a file name")?;

    let (writer, guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::never(directory, file_name));

    // An empty `RUST_LOG`, as the Nix build sets, would silence all but the
    // errors, so it counts as unset.
    let filter = std::env::var(EnvFilter::DEFAULT_ENV)
        .ok()
        .filter(|directives| !directives.trim().is_empty())
        .and_then(|directives| EnvFilter::try_new(directives).ok())
        .map_or_else(|| EnvFilter::try_new(&cfg.log_filter), Ok)
        .with_context(|| format!("invalid --log-filter {:?}", cfg.log_filter))?;

    // `try_init` rather than `init`: the integration tests start several
    // servers in one process, and only the first one installs the subscriber.
    if let Err(error) = tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false),
        )
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .try_init()
    {
        eprintln!("quiz-server: logging is already set up ({error})");
    }

    Ok(guard)
}

/// Builds the whole application.
pub fn router(state: AppState) -> Router {
    let trust_proxy = state.cfg.trust_proxy;

    let api = Router::new()
        .route("/oidc", get(api::oidc_info))
        .route("/whoami", get(api::whoami))
        .route("/room", get(api::room_status))
        .route("/room/close", post(api::close_room))
        .route("/room/award", post(api::award_points))
        // The images of a question come with it, so the request may be as
        // big as the question may get, not just the 2 MB axum allows.
        .route(
            "/questions",
            post(api::create_question).layer(DefaultBodyLimit::max(state::MAX_QUESTION_SIZE)),
        )
        .route("/questions/{id}", get(api::question))
        .route("/questions/{id}/events", get(api::question_events))
        .route("/questions/{id}/close", post(api::close_question))
        .route("/questions/{id}/reveal", post(api::reveal_question))
        .route("/questions/{id}/results", get(api::results));

    Router::new()
        // Web UI for students; every page requires an OIDC login. The room
        // is where they stay for the whole lecture, however many questions
        // the teacher asks.
        .route("/", get(web::index))
        .route("/join", post(web::join))
        .route("/r/{token}", get(web::room))
        .route("/r/{token}/answer", post(web::answer))
        .route("/r/{token}/events", get(web::events))
        // Presentation view of the room of the teacher logged in.
        .route("/d", get(display::display))
        .route("/d/events", get(display::events))
        .route("/d/close", post(display::close))
        .route("/d/reveal", post(display::reveal))
        // OIDC endpoints.
        .route("/login", get(auth::login))
        .route("/auth/callback", get(auth::callback))
        .route("/logout", get(auth::logout))
        // API for the CLI.
        .nest(API_PREFIX, api)
        .route("/static/app.css", get(stylesheet))
        .route("/static/app.js", get(javascript))
        .route("/healthz", get(|| async { "ok\n" }))
        .route("/metrics", get(metrics::metrics))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            i18n::negotiate,
        ))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(move |request: &Request<Body>| {
                    let client = if trust_proxy {
                        request
                            .headers()
                            .get("x-forwarded-for")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or("-")
                    } else {
                        "-"
                    };
                    tracing::info_span!(
                        "http",
                        method = %request.method(),
                        uri = %request.uri(),
                        client = %client,
                    )
                })
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .with_state(state)
}

/// Serves the application on an already bound listener until the process is
/// asked to terminate.
pub async fn serve(cfg: Config, listener: TcpListener) -> Result<()> {
    auth::check_config(&cfg)?;
    views::set_prefix(cfg.prefix());

    let oidc = Oidc::discover(&cfg).await?;
    tracing::info!(issuer = %cfg.oidc_issuer, "OIDC provider discovered");

    // Unpacking the syntax definitions takes a moment, and doing it while a
    // lecture hall waits for the first question would be a shame.
    tokio::task::spawn_blocking(|| {
        quiz_common::highlight::warm_up();
        tracing::info!(
            highlighting = quiz_common::highlight::is_available(),
            "syntax highlighting ready"
        );
    });

    let state = AppState::new(cfg, oidc);
    state.load_state().await;

    let pruner = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(PRUNE_INTERVAL);
        loop {
            interval.tick().await;
            pruner.prune().await;
        }
    });

    if let Ok(address) = listener.local_addr() {
        tracing::info!(%address, "listening");
    }

    // No graceful shutdown: it would wait for the event streams of open
    // pages, which never end. Apart from the optional state file there is
    // nothing to finish; returning drops every connection, and the browsers
    // reconnect once the server is back.
    let result = tokio::select! {
        result = axum::serve(listener, router(state.clone())) => {
            result.context("the HTTP server failed")
        }
        () = shutdown_signal() => Ok(()),
    };
    // Saved however the server stops, so that nothing is lost even when it
    // failed.
    let saved = state.save_state().await;
    result.and(saved)
}

/// Resolves on SIGINT or SIGTERM, so that systemd can stop the server.
async fn shutdown_signal() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => tracing::error!(%error, "cannot listen for SIGTERM"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
    tracing::info!("shutting down");
}

async fn stylesheet() -> impl IntoResponse {
    static_file("text/css; charset=utf-8", views::CSS)
}

async fn javascript() -> impl IntoResponse {
    static_file("text/javascript; charset=utf-8", views::JS)
}

/// The two static files are compiled into the binary, so that the server needs
/// nothing but its own executable to run.
fn static_file(content_type: &'static str, body: &'static str) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        body,
    )
}
