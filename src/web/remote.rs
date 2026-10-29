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
        // These handlers consume only supplied configuration/inventory/samples;
        // they cannot read store, observer or IPC paths. Preserve their complete
        // canonical projections, including successful planner diagnostics.
        if !pure_projection(path) {
            let may_write = !matches!(
                path,
                "/api/status"
                    | "/api/profiles/read"
                    | "/api/automation/status"
                    | "/api/automation/context"
                    | "/api/automation/settings/read"
            );
            redact_diagnostics(&mut body, may_write);
        }
        (status, body)
    }
}

/// Exempt only handlers whose complete call graph has no store/observer/IPC I/O.
fn pure_projection(path: &str) -> bool {
    matches!(
        path,
        "/api/inspect"
            | "/api/validate"
            | "/api/dsp/preview"
            | "/api/dsp/validate"
            | "/api/dsp/delay"
            | "/api/automation/validate"
            | "/api/automation/evaluate"
    )
}

/// Stateful APIs may describe private store/IPC paths. Keep their codes remotely
/// while retaining infrastructure details only in local diagnostics.
fn redact_diagnostics(value: &mut serde_json::Value, may_write: bool) {
    use serde_json::Value;
    match value {
        Value::Object(fields) => {
            if let (Some(Value::String(code)), Some(Value::String(message))) =
                (fields.get("code"), fields.get("message"))
            {
                let public_validation = matches!(
                    code.as_str(),
                    "invalid-json"
                        | "resource-limit"
                        | "unsupported-version"
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
                    let message = if may_write || code == "commit-uncertain" {
                        "The owning device cannot confirm this operation's outcome. A write may already have completed; inspect local diagnostics and reread authoritative state before retrying."
                    } else {
                        "The owning device could not complete this read. Inspect local diagnostics on that device and refresh authoritative state before retrying."
                    };
                    fields.insert("message".into(), Value::String(message.into()));
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
                redact_diagnostics(nested, may_write);
            }
        }
        Value::Array(values) => {
            for nested in values {
                redact_diagnostics(nested, may_write);
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
        super::redact_diagnostics(&mut reply, true);
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
        super::redact_diagnostics(&mut reply, false);
        assert!(!reply.to_string().contains("/private/account"));
        assert_eq!(reply["status"]["blocked"]["code"], "context-unavailable");
        assert_eq!(
            reply["validation"]["message"],
            "Microphone monitoring requires explicit consent"
        );
    }

    #[test]
    fn pure_graph_and_dsp_validation_keep_actionable_repair_reasons() {
        let mut reply = serde_json::json!({"error":{"code":"invalid-automation","message":"unregistered fact: power.battery-percent"},"limit":{"code":"resource-limit","message":"collection cardinality exceeds format-v1 limit"},"version":{"code":"unsupported-version","message":"unsupported graph/snapshot schema version"},"dsp":{"code":"invalid-dsp-config","message":"Channel IDs must be unique"}});
        let expected = reply.clone();
        super::redact_diagnostics(&mut reply, true);
        assert_eq!(reply, expected);
    }

    #[test]
    fn read_failures_hide_private_paths_without_claiming_a_write() {
        let mut reply = serde_json::json!({"blocked":{"code":"context-unavailable","message":"read /private/account/context: denied"}});
        super::redact_diagnostics(&mut reply, false);
        let message = reply["blocked"]["message"].as_str().unwrap();
        assert!(message.contains("read"));
        assert!(!message.contains("write"));
        assert!(!message.contains("/private/account"));
        assert!(!super::pure_projection("/api/automation/context"));
        assert!(!super::pure_projection("/api/profiles/read"));
        assert!(super::pure_projection("/api/inspect"));
    }
}
