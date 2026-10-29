//! Authenticated transports enter the same typed policy/store handlers as local HTTP.
//! This adapter owns the automation worker: do not run another owner on this store.
use super::{activation, api, assets, State, MAX_RESPONSE};
use mixyard::{profiles, runtime};
use std::{path::Path, sync::Arc};
use tokio::sync::Semaphore;

pub struct Canonical {
    state: State,
}

impl Canonical {
    pub fn new(
        store: &Path,
        assets: &Path,
        instance: String,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        runtime::endpoint(&instance)?;
        // Validate before starting the owner thread or touching automation state.
        let profile_path = store;
        let store = profiles::Store::new(profile_path)?;
        store.read_bounded(MAX_RESPONSE)?;
        let assets = assets::Assets::load(assets)?;
        let activation = activation::Service::start(profile_path, instance.clone());
        Ok(Self {
            state: State {
                authority: String::new(),
                token: String::new(),
                instance,
                store,
                assets,
                activation,
                operations: Arc::new(Semaphore::new(4)),
            },
        })
    }

    pub fn revision(&self) -> Result<u64, String> {
        self.state
            .store
            .read_bounded(MAX_RESPONSE)
            .map(|s| s.store_revision)
            .map_err(|e| {
                eprintln!("Remote profile store: {e}");
                "Profile store unavailable. Inspect local diagnostics on the owning device.".into()
            })
    }

    /// Call only after identity/session/role admission. No HTTP CSRF token is auth.
    pub fn dispatch(&self, path: &str, body: &[u8]) -> (u16, serde_json::Value) {
        let response = api::dispatch(path, body, &self.state);
        let status = response.status().as_u16();
        // Canonical replies are already fully serialized, bounded bodies; no I/O.
        let bytes = response
            .into_body()
            .into_inner()
            .expect("canonical JSON body");
        let mut body = serde_json::from_slice(&bytes).expect("canonical JSON reply");
        redact_diagnostics(&mut body);
        (status, body)
    }
}

/// Local APIs intentionally describe private store/IPC paths. Keep their codes
/// remotely, while retaining infrastructure details only in local diagnostics.
fn redact_diagnostics(value: &mut serde_json::Value) {
    use serde_json::Value;
    match value {
        Value::Object(fields) => {
            if let (Some(Value::String(code)), Some(Value::String(message))) =
                (fields.get("code"), fields.get("message"))
            {
                let public_validation = matches!(
                    code.as_str(),
                    "invalid-json"
                        | "invalid-automation"
                        | "invalid-dsp-config"
                        | "invalid-dsp-signal"
                        | "dsp-preview-budget"
                        | "dsp-alignment-selection"
                        | "dsp-delay-budget"
                        | "dsp-delay-rejected"
                        | "dsp-processing-failed"
                        | "invalid-policy"
                        | "invalid-settings"
                        | "invalid-name"
                        | "invalid-observer-options"
                        | "store-conflict"
                        | "profile-not-found"
                        | "no-undo"
                        | "revision-exhausted"
                        | "inspection-too-large"
                );
                if !public_validation {
                    eprintln!("Remote canonical {code}: {message}");
                    fields.insert("message".into(), Value::String("The owning device cannot confirm this operation's outcome. A write may already have completed; inspect local diagnostics and reread authoritative state before retrying.".into()));
                }
            }
            // Read-only runtime status uses a plain error string rather than a
            // structured failure, and may include a private IPC endpoint.
            if let Some(Value::String(message)) = fields.get_mut("error") {
                eprintln!("Remote runtime status: {message}");
                *message =
                    "Runtime unavailable. Inspect local diagnostics on the owning device.".into();
            }
            for nested in fields.values_mut() {
                redact_diagnostics(nested);
            }
        }
        Value::Array(values) => {
            for nested in values {
                redact_diagnostics(nested);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn uncertain_writes_remain_explicitly_uncertain_after_redaction() {
        let mut reply = serde_json::json!({"error":{"code":"commit-uncertain","message":"Replacement occurred but sync /private/account/profiles failed"}});
        super::redact_diagnostics(&mut reply);
        assert_eq!(reply["error"]["code"], "commit-uncertain");
        assert!(reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("may already have completed"));
        assert!(reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("reread authoritative state"));
        assert!(!reply.to_string().contains("/private/account"));
    }

    #[test]
    fn nested_failures_hide_private_paths_without_changing_validation() {
        let mut reply = serde_json::json!({"status":{"blocked":{"code":"context-unavailable","message":"read /private/account/context: denied"}},"error":"IPC /private/account/runtime failed","validation":{"code":"invalid-policy","message":"Microphone monitoring requires explicit consent"}});
        super::redact_diagnostics(&mut reply);
        assert!(!reply.to_string().contains("/private/account"));
        assert_eq!(reply["status"]["blocked"]["code"], "context-unavailable");
        assert_eq!(
            reply["validation"]["message"],
            "Microphone monitoring requires explicit consent"
        );
    }

    #[test]
    fn pure_graph_and_dsp_validation_keep_actionable_repair_reasons() {
        let mut reply = serde_json::json!({"error":{"code":"invalid-automation","message":"unregistered fact: power.battery-percent"},"dsp":{"code":"invalid-dsp-config","message":"Channel IDs must be unique"}});
        let expected = reply.clone();
        super::redact_diagnostics(&mut reply);
        assert_eq!(reply, expected);
    }
}
