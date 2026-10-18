# The quiz server and the teacher CLI, with shell completions for the CLI.
{ lib, rustPlatform, installShellFiles }:

rustPlatform.buildRustPackage {
  pname = "quiz";
  version = (lib.importTOML ../Cargo.toml).workspace.package.version;

  # Only what Cargo needs, so that editing the Nix files or the deployment
  # examples does not rebuild the package.
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../common
      ../server
      ../cli
      ../testsupport
    ];
  };
  # No vendor hash needed: the lock file pins everything.
  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [ installShellFiles ];

  # `cli/build.rs` writes the completions here while it compiles.
  preBuild = ''
    export QUIZ_COMPLETIONS_DIR="$NIX_BUILD_TOP/completions"
  '';

  postInstall = ''
    installShellCompletion \
      --bash "$QUIZ_COMPLETIONS_DIR/quiz.bash" \
      --zsh  "$QUIZ_COMPLETIONS_DIR/_quiz" \
      --fish "$QUIZ_COMPLETIONS_DIR/quiz.fish"
  '';

  # The tests start the server and a mock OIDC provider on loopback,
  # which works inside the build sandbox.
  meta = with lib; {
    description = "Quiz server and teacher CLI for asking students questions";
    mainProgram = "quiz-server";
    license = licenses.mit;
    platforms = platforms.unix;
  };
}
