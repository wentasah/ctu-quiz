//! A development OIDC provider, for trying the quiz app out on one machine.
//!
//! It speaks real OpenID Connect - discovery document, JWKS, authorization
//! code flow with PKCE, RS256 ID tokens - but it asks for no password: you
//! type a login into a form and get a token for it. That makes it useful for
//! a local walk-through and useless for anything else.
//!
//!     cargo run -p quiz-testsupport --bin mock-idp

use clap::Parser;
use quiz_testsupport::{LoginMode, MockIdp};

/// Development-only OIDC provider for the quiz app.
#[derive(Debug, Parser)]
#[command(name = "mock-idp", version, about, long_about = None)]
struct Args {
    /// Address to listen on. Use the address of your machine on the local
    /// network (for example 192.168.1.5:9000) if you want to log in from a
    /// phone; the issuer URL follows from it.
    #[arg(long, default_value = "127.0.0.1:9000")]
    listen: String,
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();
    let idp = MockIdp::listen(&args.listen, LoginMode::Interactive).await?;
    let issuer = &idp.issuer;

    println!(
        "\
Development OIDC provider listening on {issuer}

  !! It issues a token for any login, without a password. !!
  !! Use it on your own machine only.                     !!

  discovery: {issuer}/.well-known/openid-configuration

Start the quiz server (in another terminal):

  cargo run -p quiz-server -- \\
      --oidc-issuer {issuer} \\
      --oidc-client-id quiz \\
      --teachers sojkam1

and ask a question (in a third one):

  cargo run -p quiz-cli --bin quiz -- ask question.md

Press Ctrl-C to stop."
    );

    tokio::signal::ctrl_c().await?;
    println!("\nStopped.");
    Ok(())
}
