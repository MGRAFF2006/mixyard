use mixyard::{plan, plan_media, Config, Inventory, MediaRequest};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::{
    env,
    ffi::OsString,
    fs::File,
    io::{self, BufReader, Write},
    path::Path,
    process::ExitCode,
};

// Exit 0 means completed validation/planning, including a plan with disconnected
// sources. It never attests to native audio. Errors are one JSON object on stderr;
// normal results are one JSON object on stdout. Inspection also writes its
// report on preflight rejection (exit 3). Help is plain text.
// 2: usage/unsupported command, 3: invalid JSON/policy, 4: input/output failure.
const USAGE: &str = "Usage: mixyard validate CONFIG.json\n       mixyard plan CONFIG.json INVENTORY.json\n       mixyard inspect CONFIG.json INVENTORY.json\n       mixyard explain CONFIG.json INVENTORY.json\n       mixyard plan-media CONFIG.json INVENTORY.json REQUEST.json\nRead-only routing/media planning. No OS audio changes or media dispatch.\nExit codes: 0 completed (including degraded/unavailable plans), 2 usage/unsupported command, 3 invalid input/policy, 4 I/O failure.";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CliError {
    schema_version: u8,
    code: &'static str,
    message: String,
    exit_code: u8,
}

impl CliError {
    fn new(code: &'static str, message: impl Into<String>, exit_code: u8) -> Self {
        Self {
            schema_version: 2,
            code,
            message: message.into(),
            exit_code,
        }
    }

    fn io(error: impl std::fmt::Display) -> Self {
        Self::new("io-error", error.to_string(), 4)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Validation {
    schema_version: u8,
    mode: &'static str,
    valid: bool,
    config_revision: u64,
    mix_count: usize,
    output_count: usize,
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, CliError> {
    let file =
        File::open(path).map_err(|error| CliError::io(format!("{}: {error}", path.display())))?;
    serde_json::from_reader(BufReader::new(file)).map_err(|error| {
        let message = format!("{}: {error}", path.display());
        if error.is_io() {
            CliError::io(message)
        } else {
            CliError::new("invalid-json", message, 3)
        }
    })
}

fn write_json(output: &mut impl Write, value: &impl Serialize) -> Result<(), CliError> {
    serde_json::to_writer_pretty(&mut *output, value).map_err(CliError::io)?;
    writeln!(output).map_err(CliError::io)
}

fn run(args: &[OsString], output: &mut impl Write) -> Result<(), CliError> {
    let Some(command) = args.first() else {
        return Err(CliError::new("usage", USAGE, 2));
    };
    if command == "--help" || command == "-h" {
        if args.len() != 1 {
            return Err(CliError::new("usage", USAGE, 2));
        }
        return writeln!(output, "{USAGE}").map_err(CliError::io);
    }
    let expected = if command == "validate" {
        2
    } else if command == "plan" || command == "inspect" || command == "explain" {
        3
    } else if command == "plan-media" {
        4
    } else {
        return Err(CliError::new("unsupported-command", "Command unavailable; only validate, plan, plan-media, inspect and explain are implemented. No mutation, media dispatch or runtime connection was attempted.", 2));
    };
    if args.len() != expected {
        return Err(CliError::new("usage", USAGE, 2));
    }
    let config: Config = read_json(Path::new(&args[1]))?;
    if command == "validate" {
        config
            .validate()
            .map_err(|error| CliError::new("invalid-policy", error, 3))?;
        return write_json(
            output,
            &Validation {
                schema_version: 2,
                mode: "validate",
                valid: true,
                config_revision: config.revision,
                mix_count: config.mixes.len(),
                output_count: config.outputs.len(),
            },
        );
    }
    let inventory: Inventory = read_json(Path::new(&args[2]))?;
    if command == "inspect" || command == "explain" {
        let inspection = mixyard::diagnostics::inspect(&config, &inventory);
        write_json(output, &inspection)?;
        return if inspection.plan_available {
            Ok(())
        } else {
            Err(CliError::new("inspection-preflight-failed", "Inspection written to stdout; authoritative planner rejected input. No routing attempted.", 3))
        };
    }
    if command == "plan-media" {
        let request: MediaRequest = read_json(Path::new(&args[3]))?;
        let result = plan_media(&config, &inventory, &request)
            .map_err(|error| CliError::new("invalid-policy", error, 3))?;
        return write_json(output, &result);
    }
    let result =
        plan(&config, &inventory).map_err(|error| CliError::new("invalid-policy", error, 3))?;
    write_json(output, &result)
}

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    match run(&args, &mut io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // If stderr itself fails there is no further channel to report it;
            // still preserve the classified failure exit code.
            if write_json(&mut io::stderr().lock(), &error).is_err() {
                return ExitCode::from(4);
            }
            ExitCode::from(error.exit_code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_runtime_commands_fail_without_output() {
        for command in ["apply", "status", "mute", "events"] {
            let mut output = Vec::new();
            let error = run(&[command.into()], &mut output).unwrap_err();
            assert_eq!(error.code, "unsupported-command");
            assert_eq!(error.exit_code, 2);
            assert!(output.is_empty());
            let json = serde_json::to_value(&error).unwrap();
            assert_eq!(json["schemaVersion"], 2);
            assert_eq!(json["exitCode"], 2);
        }
    }

    #[test]
    fn argument_errors_precede_file_access() {
        for args in [
            vec![],
            vec!["validate".into()],
            vec!["plan".into(), "absent.json".into()],
        ] {
            let mut output = Vec::new();
            let error = run(&args, &mut output).unwrap_err();
            assert_eq!(error.code, "usage");
            assert_eq!(error.exit_code, 2);
            assert!(output.is_empty());
        }
    }
}
