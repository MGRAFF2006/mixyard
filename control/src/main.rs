use mixyard_control::{Control, IdentityStore, Options, Permission};
use std::{collections::BTreeMap, io::Write, path::PathBuf};

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("mixyard-control: {error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut values = BTreeMap::new();
    let mut command = None;
    let mut enabled = false;
    while let Some(arg) = args.next() {
        if arg == "--help" || arg == "-h" {
            println!("mixyard-control --state DIRECTORY init --ip PRIVATE_IP\n  trust\n  invite --permissions view,control,settings [--ttl-seconds 300]\n  revoke --credential-id ID (server stopped)\n  serve --enable-control --bind PRIVATE_IP:PORT --store DIRECTORY --assets DIRECTORY [--instance NAME]\nTLS authenticated stored-intent only. No native audio. No wildcard/public listener.");
            return Ok(());
        }
        if arg == "--enable-control" {
            if enabled {
                return Err("Duplicate opt-in flag".into());
            }
            enabled = true;
        } else if arg.starts_with("--") {
            if ![
                "--state",
                "--ip",
                "--permissions",
                "--ttl-seconds",
                "--credential-id",
                "--bind",
                "--store",
                "--assets",
                "--instance",
            ]
            .contains(&arg.as_str())
            {
                return Err(format!("Unknown option {arg}").into());
            }
            let value = args.next().ok_or("Missing option value")?;
            if values.insert(arg, value).is_some() {
                return Err("Duplicate option".into());
            }
        } else if command.replace(arg).is_some() {
            return Err("Only one command is allowed".into());
        }
    }
    let mut take = |key: &str| {
        values
            .remove(key)
            .ok_or_else(|| format!("Required option {key}"))
    };
    let state = PathBuf::from(take("--state")?);
    let command = command.ok_or("Choose init, trust, invite, revoke or serve")?;
    let initialize = if command == "init" {
        Some(take("--ip")?.parse()?)
    } else {
        None
    };
    let mut identity = IdentityStore::open(&state, initialize)?;
    let output = match command.as_str() {
        "init" | "trust" => serde_json::to_value(identity.trust())?,
        "invite" => {
            let roles = values
                .remove("--permissions")
                .ok_or("Required --permissions")?;
            let permissions = roles
                .split(',')
                .map(|p| match p {
                    "view" => Ok(Permission::View),
                    "control" => Ok(Permission::Control),
                    "settings" => Ok(Permission::Settings),
                    _ => Err("Unknown permission"),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let ttl = values
                .remove("--ttl-seconds")
                .unwrap_or_else(|| "300".into())
                .parse()?;
            identity.invite(permissions, ttl)?
        }
        "revoke" => {
            identity.revoke(
                &values
                    .remove("--credential-id")
                    .ok_or("Required --credential-id")?,
            )?;
            serde_json::json!({"revoked":true})
        }
        "serve" => {
            let options = Options {
                enabled,
                bind: values.remove("--bind").ok_or("Required --bind")?.parse()?,
                store: values.remove("--store").ok_or("Required --store")?.into(),
                assets: values.remove("--assets").ok_or("Required --assets")?.into(),
                instance: values
                    .remove("--instance")
                    .unwrap_or_else(|| "default".into()),
            };
            if !values.is_empty() {
                return Err("Options do not apply to serve".into());
            }
            let server = Control::bind(identity, options).await?;
            println!("{}", server.readiness());
            std::io::stdout().flush()?;
            #[cfg(unix)]
            let shutdown = async {
                let mut terminate =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("SIGTERM handler");
                tokio::select! { _=terminate.recv()=>{}, _=tokio::signal::ctrl_c()=>{} }
            };
            #[cfg(not(unix))]
            let shutdown = async {
                let _ = tokio::signal::ctrl_c().await;
            };
            return Ok(server.serve_until(shutdown).await?);
        }
        _ => return Err("Unknown command".into()),
    };
    if !values.is_empty() || enabled {
        return Err("Options do not apply to this command".into());
    }
    println!("{output}");
    Ok(())
}
