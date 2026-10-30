use std::process::ExitCode;
fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--help"] || args == ["-h"] {
        println!("Usage: mixyard-runtime serve|inventory [--instance NAME]\n       mixyard-runtime serve [--instance NAME] --inventory-fixture-helper /ABSOLUTE/HELPER\nRead-only PipeWire/installed Windows/CoreAudio discovery; fixture helpers always produce simulated evidence. No audio backend or automatic startup.");
        return ExitCode::SUCCESS;
    }
    let mut instance = "default";
    let mut instance_seen = false;
    let mut fixture_helper = None;
    let mut private_descriptor = None;
    let mut profiles = None;
    let command = args.first().map(String::as_str).unwrap_or("");
    if !matches!(command, "serve" | "inventory") {
        return ExitCode::from(2);
    }
    let (flags, remainder) = args[1..].as_chunks::<2>();
    for flag in flags {
        match flag[0].as_str() {
            "--instance" if !instance_seen => {
                instance = &flag[1];
                instance_seen = true;
            }
            "--inventory-fixture-helper"
                if command == "serve"
                    && fixture_helper.is_none()
                    && std::path::Path::new(&flag[1]).is_absolute() =>
            {
                fixture_helper = Some(std::path::PathBuf::from(&flag[1]))
            }
            "--owned-private-audio-descriptor"
                if cfg!(target_os = "linux")
                    && command == "serve"
                    && private_descriptor.is_none()
                    && std::path::Path::new(&flag[1]).is_absolute() =>
            {
                private_descriptor = Some(std::path::PathBuf::from(&flag[1]));
            }
            "--profiles"
                if cfg!(target_os = "linux")
                    && command == "serve"
                    && profiles.is_none()
                    && std::path::Path::new(&flag[1]).is_absolute() =>
            {
                profiles = Some(std::path::PathBuf::from(&flag[1]));
            }
            _ => return ExitCode::from(2),
        }
    }
    if !remainder.is_empty() || private_descriptor.is_some() != profiles.is_some() {
        return ExitCode::from(2);
    }
    let result = if command == "inventory" {
        mixyard::runtime::client(instance, "inventory", &mut std::io::stdout().lock())
    } else {
        #[cfg(target_os = "linux")]
        {
            mixyard::runtime::serve_with_private_audio(
                instance,
                mixyard::runtime::inventory::Options { fixture_helper },
                private_descriptor
                    .zip(profiles)
                    .map(
                        |(descriptor, profiles)| mixyard::runtime::private_audio::Options {
                            descriptor,
                            profiles,
                        },
                    ),
            )
        }
        #[cfg(not(target_os = "linux"))]
        mixyard::runtime::serve_with_inventory(
            instance,
            mixyard::runtime::inventory::Options { fixture_helper },
        )
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!(
                "{}",
                serde_json::json!({"ipcVersion":1,"requestId":null,"error":{"code":"runtime-io-error","message":e.to_string()}})
            );
            ExitCode::from(4)
        }
    }
}
