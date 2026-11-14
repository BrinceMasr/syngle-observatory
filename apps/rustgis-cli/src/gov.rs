//! `gov`: the Australian government data lookup (`syngle-map gov abn|name|company|business-names|refresh|status ...`,
//! Syngle-Ecosystem #628 to #630). All of it is in `observatory-gov`; this prints the one line of JSON it answers with and
//! exits with its code (0 answered, 2 refused). It runs before the plugins and the session are loaded: a lookup is one process
//! per call, so start-up is part of its time, and it must answer even where an argument is not valid UTF-8.

use observatory_gov::{Env, run};
use std::io::Write;
use std::process::ExitCode;

const INTERNAL: &str =
    "{\"ok\":false,\"error\":{\"code\":\"refresh-failed\",\"message\":\"internal error; this is a bug, please report it\"}}\n";

pub fn main() -> ExitCode {
    let args: Vec<String> = std::env::args_os().skip(2).map(|a| a.to_string_lossy().into_owned()).collect();
    let started = std::time::Instant::now();
    let (line, exit) = match std::panic::catch_unwind(|| run(&args, &Env::from_process())) {
        Ok(outcome) => (outcome.stdout, outcome.exit),
        Err(_) => (INTERNAL.to_string(), 2),
    };
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(line.as_bytes());
    let _ = out.flush();
    if args.iter().any(|a| a == "--timing") {
        eprintln!("gov: {:.1} ms", started.elapsed().as_secs_f64() * 1000.0);
    }
    ExitCode::from(exit)
}
