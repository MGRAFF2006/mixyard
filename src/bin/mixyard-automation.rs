//! Explicit snapshots only. This binary does not inspect devices/processes or write profiles.
use mixyard::automation::{evaluate, parse_request, standard_catalog, Error, MAX_REQUEST_BYTES};
use std::{
    fs::File,
    io::{self, Read, Write},
    process::ExitCode,
};

fn run() -> Result<(), (u8, Error)> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let output = match args.as_slice() {
        [command] if command == "catalog" => serde_json::to_value(standard_catalog()),
        [command, path] if command == "evaluate" => {
            let file = File::open(path).map_err(|e| (4, error("io-error", e)))?;
            let mut bytes = vec![];
            file.take(MAX_REQUEST_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|e| (4, error("io-error", e)))?;
            let request = parse_request(&bytes).map_err(|e| (3, e))?;
            let result = evaluate(&request).map_err(|e| (3, e))?;
            serde_json::to_value(result)
        }
        _ => return Err((2, error("usage", "mixyard-automation catalog | evaluate REQUEST.json (read-only, no live observation)"))),
    }.map_err(|e| (3, error("serialization-error", e)))?;
    let mut stdout = io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, &output).map_err(|e| (4, error("io-error", e)))?;
    writeln!(stdout).map_err(|e| (4, error("io-error", e)))
}
fn error(code: &'static str, message: impl std::fmt::Display) -> Error {
    Error {
        code,
        message: message.to_string(),
    }
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err((exit, error)) => {
            eprintln!(
                "{}",
                serde_json::json!({"schemaVersion":1,"error":error,"audioApplied":false})
            );
            ExitCode::from(exit)
        }
    }
}
