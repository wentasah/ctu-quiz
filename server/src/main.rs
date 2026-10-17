//! Entry point of the quiz server.

use std::process::ExitCode;

use anyhow::Context;
use clap::Parser;
use quiz_server::{config::Config, init_logging, serve};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // The logger may not be up yet, so report on stderr as well.
            eprintln!("quiz-server: {error:#}");
            tracing::error!(error = %format!("{error:#}"), "the server stopped");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> anyhow::Result<()> {
    let cfg = Config::parse().normalize()?;
    // Held until the end of main so that the log file gets flushed.
    let _log_guard = init_logging(&cfg)?;

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        listen = %cfg.listen,
        public_url = %cfg.public_url,
        issuer = %cfg.oidc_issuer,
        client_id = %cfg.oidc_client_id,
        teachers = ?cfg.teachers,
        teacher_roles = ?cfg.teacher_roles,
        roles_claim = %cfg.roles_claim,
        allow_any_teacher = cfg.allow_any_teacher,
        log_file = ?cfg.log_file,
        "starting the quiz server"
    );

    // Bind before contacting the provider, so that a port clash is reported
    // right away.
    let listener = TcpListener::bind(cfg.listen)
        .await
        .with_context(|| format!("cannot listen on {}", cfg.listen))?;

    serve(cfg, listener).await
}
