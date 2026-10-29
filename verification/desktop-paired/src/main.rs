//! Owned API double for the vault only. TLS and all pairing/session logic are production code.
use mixyard_desktop_control::{Paired, PlatformVault, Vault};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{BufRead, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
#[derive(Clone, Default)]
struct TestVault {
    values: Arc<Mutex<BTreeMap<String, String>>>,
    fail: Arc<AtomicBool>,
}
impl Vault for TestVault {
    fn read(&self, key: &str) -> Result<Option<String>, String> {
        if self.fail.load(Ordering::SeqCst) {
            Err("Injected vault unavailable".into())
        } else {
            Ok(self.values.lock().unwrap().get(key).cloned())
        }
    }
    fn write(&self, key: &str, value: &str) -> Result<(), String> {
        if self.fail.load(Ordering::SeqCst) {
            Err("Injected vault unavailable".into())
        } else {
            self.values.lock().unwrap().insert(key.into(), value.into());
            Ok(())
        }
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        if self.fail.load(Ordering::SeqCst) {
            Err("Injected vault unavailable".into())
        } else {
            self.values.lock().unwrap().remove(key);
            Ok(())
        }
    }
}
fn main() {
    let vault = TestVault::default();
    let mut client = Paired::new(vault.clone());
    for line in std::io::stdin().lock().lines() {
        let result = (|| -> Result<Value, String> {
            let value: Value = serde_json::from_str(&line.map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            let text = |key: &str| value[key].as_str().ok_or_else(|| format!("Missing {key}"));
            match text("op")? {
                "pair" => serde_json::to_value(
                    client.pair(
                        text("endpoint")?.into(),
                        text("name")?.into(),
                        serde_json::from_value(value["invitation"].clone())
                            .map_err(|e| e.to_string())?,
                    )?,
                )
                .map_err(|e| e.to_string()),
                "list" => serde_json::to_value(client.list()?).map_err(|e| e.to_string()),
                "open" => serde_json::to_value(client.open(text("id")?, text("credentialId")?)?)
                    .map_err(|e| e.to_string()),
                "request" => serde_json::to_value(client.request(
                    text("id")?,
                    value["generation"].as_u64().ok_or("Missing generation")?,
                    text("raw")?,
                )?)
                .map_err(|e| e.to_string()),
                "disconnect" => {
                    client.disconnect(text("id")?, value["generation"].as_u64())?;
                    Ok(json!({"disconnected":true}))
                }
                "forget" => {
                    client.forget(text("id")?)?;
                    Ok(json!({"forgotten":true}))
                }
                "revoke" => serde_json::to_value(client.revoke_self(
                    text("id")?,
                    value["generation"].as_u64().ok_or("Missing generation")?,
                )?)
                .map_err(|e| e.to_string()),
                "restart" => {
                    client = Paired::new(vault.clone());
                    Ok(json!({"restarted":true}))
                }
                "vault-fail" => {
                    vault
                        .fail
                        .store(value["fail"].as_bool().unwrap(), Ordering::SeqCst);
                    Ok(json!({"injected":true}))
                }
                "platform-vault-unavailable" => {
                    let directory = std::env::temp_dir()
                        .join(format!("mixyard-vault-failure-{}", std::process::id()));
                    let vault = PlatformVault::open(Ok(directory.clone()));
                    let unavailable = vault.read("qualification-no-secret").is_err();
                    drop(vault);
                    std::fs::remove_dir_all(directory)
                        .map_err(|_| "Owned platform-vault lock cleanup failed")?;
                    if !unavailable {
                        Err("Expected unavailable isolated platform vault".into())
                    } else {
                        Ok(json!({"failedClosed":true}))
                    }
                }
                _ => Err("Unsupported qualification operation".into()),
            }
        })();
        println!(
            "{}",
            match result {
                Ok(value) => json!({"ok":value}),
                Err(error) => json!({"error":error}),
            }
        );
        std::io::stdout().flush().unwrap();
    }
}
