//! Writes the shell completion scripts while the CLI is compiled.
//!
//! `src/cli.rs` holds the clap definition and nothing else, so it can be
//! included here and turned into completions without the binary having to be
//! built and run first. The scripts land in `$OUT_DIR/completions`, and also
//! in `$QUIZ_COMPLETIONS_DIR` when that is set, which is what packaging
//! installs from (see `flake.nix`).

use std::{env, fs, io};

use clap::{CommandFactory, ValueEnum};
use clap_complete::{Shell, generate_to};

// Brings `Cli` in, together with the imports it needs (`PathBuf`, `Url`).
include!("src/cli.rs");

fn main() -> io::Result<()> {
    println!("cargo::rerun-if-changed=src/cli.rs");
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=QUIZ_COMPLETIONS_DIR");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let mut directories = vec![out_dir.join("completions")];
    directories.extend(env::var_os("QUIZ_COMPLETIONS_DIR").map(PathBuf::from));

    let mut command = Cli::command();
    for directory in directories {
        fs::create_dir_all(&directory)?;
        for &shell in Shell::value_variants() {
            generate_to(shell, &mut command, "quiz", &directory)?;
        }
    }
    Ok(())
}
