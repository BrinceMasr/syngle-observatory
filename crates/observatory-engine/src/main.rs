//! The Syngle Observatory engine sidecar's entry point.
//!
//! All of the work lives in the `observatory_engine` library, so it can be tested without a
//! process; this binary only parses the command line and returns the library's exit code.
//!
//! Usage: `syngle-observatory-engine --root <repo> --data <dir> [--web <dir>] [--preload <url|path>]`

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(observatory_engine::run(&args));
}
