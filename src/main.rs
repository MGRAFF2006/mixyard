use mixyard::{
    plan, plan_media,
    profiles::{self, Edit, Store},
    Config, Inventory, MediaRequest,
};
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
// normal validate/plan results are one JSON object on stdout. Help is plain text.
// 2: usage/unsupported command, 3: invalid JSON/policy, 4: input/output failure.
const USAGE: &str = "Usage: mixyard validate CONFIG.json\n       mixyard plan CONFIG.json INVENTORY.json\n       mixyard plan-media CONFIG.json INVENTORY.json REQUEST.json\n       mixyard profiles [--store DIRECTORY] COMMAND ...\nLocal profiles/drafts: list, show, save REV NAME CONFIG [ID], rename REV ID NAME, draft REV CONFIG, load REV ID, undo REV, preview CONFIG INVENTORY, preview-profile ID INVENTORY, preview-undo INVENTORY.\nNo OS audio changes or media dispatch.\nExit codes: 0 completed (including degraded/unavailable plans), 2 usage/unsupported command, 3 invalid input/policy, 4 I/O failure, 5 store conflict/busy/recovery.";

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
    if command == "profiles" {
        return run_profiles(&args[1..], output);
    }
    let expected = if command == "validate" {
        2
    } else if command == "plan" {
        3
    } else if command == "plan-media" {
        4
    } else {
        return Err(CliError::new("unsupported-command", "Command unavailable; only validate, plan, plan-media and local profiles are implemented. No audio mutation, media dispatch or runtime connection was attempted.", 2));
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

impl From<profiles::Error> for CliError {
    fn from(error: profiles::Error) -> Self {
        Self::new(error.code, error.message, error.exit_code)
    }
}

fn run_profiles(args: &[OsString], output: &mut impl Write) -> Result<(), CliError> {
    let (store_path, args) = if args.first().is_some_and(|a| a == "--store") {
        if args.len() < 3 {
            return Err(CliError::new("usage", USAGE, 2));
        }
        (Some(Path::new(&args[1]).to_owned()), &args[2..])
    } else {
        (None, args)
    };
    let command = args.first().and_then(|a| a.to_str()).unwrap_or("");
    let valid_arity = match command {
        "list" | "show" => args.len() == 1,
        "save" => matches!(args.len(), 4 | 5),
        "rename" => args.len() == 4,
        "draft" | "load" | "preview" | "preview-profile" => args.len() == 3,
        "undo" | "preview-undo" => args.len() == 2,
        _ => false,
    };
    if !valid_arity {
        return Err(CliError::new("usage", USAGE, 2));
    }
    let text = |i: usize| -> Result<String, CliError> {
        args[i].to_str().map(str::to_owned).ok_or_else(|| {
            CliError::new("usage", "Profile names, IDs and revisions must be UTF-8", 2)
        })
    };
    let expected = if matches!(command, "save" | "rename" | "draft" | "load" | "undo") {
        Some(text(1)?.parse::<u64>().map_err(|_| {
            CliError::new("usage", "Expected store revision must be a u64 integer", 2)
        })?)
    } else {
        None
    };
    let store = Store::new(match store_path {
        Some(p) => p,
        None => profiles::default_directory()?,
    })?;
    let snapshot = store.read()?;
    let (revision, result, sensitive) = match command {
        "preview" | "preview-profile" | "preview-undo" => {
            let candidate = match command {
                "preview" => read_json::<Config>(Path::new(&args[1]))?,
                "preview-profile" => snapshot.profile(&text(1)?)?.config.clone(),
                _ => snapshot
                    .previous_draft
                    .clone()
                    .ok_or_else(|| CliError::new("no-undo", "No previous validated draft", 3))?,
            };
            let inventory: Inventory = read_json(Path::new(
                &args[if command == "preview-undo" { 1 } else { 2 }],
            ))?;
            let preview = profiles::preview(&snapshot, &candidate, &inventory)?;
            let sensitive = serde_json::to_value(&preview.sensitive_paths).map_err(CliError::io)?;
            (
                snapshot.store_revision,
                serde_json::to_value(preview).map_err(CliError::io)?,
                sensitive,
            )
        }
        "list" | "show" => {
            let sensitive = snapshot
                .draft
                .as_ref()
                .map(profiles::sensitive_paths)
                .unwrap_or_default();
            let result = if command == "show" {
                serde_json::to_value(&snapshot).map_err(CliError::io)?
            } else {
                serde_json::json!({"profiles": snapshot.profiles.iter().map(|p| serde_json::json!({"id": p.id, "name": p.name})).collect::<Vec<_>>(),
                    "draftRevision": snapshot.draft.as_ref().map(|c| c.revision), "undoAvailable": snapshot.previous_draft.is_some()})
            };
            (
                snapshot.store_revision,
                result,
                serde_json::to_value(sensitive).map_err(CliError::io)?,
            )
        }
        _ => {
            let edit = match command {
                "save" => Edit::Save {
                    id: if args.len() == 5 {
                        Some(text(4)?)
                    } else {
                        None
                    },
                    name: text(2)?,
                    config: read_json(Path::new(&args[3]))?,
                },
                "rename" => Edit::Rename {
                    id: text(2)?,
                    name: text(3)?,
                },
                "draft" => Edit::Draft(read_json(Path::new(&args[2]))?),
                "load" => Edit::Load { id: text(2)? },
                _ => Edit::Undo,
            };
            let changed = store.edit(expected.unwrap(), edit)?;
            let config = if command == "save" {
                if args.len() == 5 {
                    Some(&changed.profile(&text(4)?)?.config)
                } else {
                    changed.profiles.last().map(|p| &p.config)
                }
            } else {
                changed.draft.as_ref()
            };
            let sensitive = config.map(profiles::sensitive_paths).unwrap_or_default();
            (
                changed.store_revision,
                serde_json::to_value(&changed).map_err(CliError::io)?,
                serde_json::to_value(sensitive).map_err(CliError::io)?,
            )
        }
    };
    let review_required = !sensitive.as_array().unwrap().is_empty()
        || result["affectedSensitivePathsBefore"]
            .as_array()
            .is_some_and(|paths| !paths.is_empty());
    write_json(
        output,
        &serde_json::json!({
            "schemaVersion": 1, "mode": "local-profiles", "command": command,
            "storeRevision": revision, "result": result, "audioApplied": false,
            "confirmedWorkingSetup": null, "storedConsentIsFreshNativeConsent": false,
            "reviewRequiredBeforeApply": review_required,
            "sensitivePaths": sensitive,
        }),
    )
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
