//! Content-only desktop boundary for the existing read-only core inspector.
use mixyard::{diagnostics, Config, Inventory};
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

pub const MAX_INPUT_BYTES: usize = 131_072;
const MAX_RESPONSE_BYTES: usize = 2_097_152;
const MAX_SAFE_REVISION: u64 = 9_007_199_254_740_991;
static BUSY: AtomicBool = AtomicBool::new(false);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotRequest {
    pub config_json: String,
    pub inventory_json: String,
}

pub struct Admission;
impl Drop for Admission {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

pub fn acquire() -> Result<Admission, String> {
    BUSY.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .map(|_| Admission)
        .map_err(|_| "An inspection is already running. Try again when it completes.".into())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportedInspection<'a> {
    schema_version: u8,
    provenance: &'static str,
    freshness: &'static str,
    report: diagnostics::Inspection<'a>,
}

// Stop serialization at the boundary rather than allocate an unbounded IPC reply.
struct BoundedReply(Vec<u8>);
impl Write for BoundedReply {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.len() + bytes.len() > MAX_RESPONSE_BYTES {
            return Err(io::Error::other(
                "Inspection exceeds the 2 MiB reply limit.",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn inspect(request: SnapshotRequest) -> Result<String, String> {
    for (label, content) in [
        ("Config", &request.config_json),
        ("Inventory", &request.inventory_json),
    ] {
        if content.len() > MAX_INPUT_BYTES {
            return Err(format!("{label} exceeds the 128 KiB import limit."));
        }
    }
    let config: Config = serde_json::from_str(&request.config_json)
        .map_err(|error| format!("Config JSON rejected: {error}"))?;
    let inventory: Inventory = serde_json::from_str(&request.inventory_json)
        .map_err(|error| format!("Inventory JSON rejected: {error}"))?;
    if config.schema_version != 2 || inventory.schema_version != 2 {
        return Err("Import requires schema-2 config and inventory; discovery envelope 1 is not an inventory.".into());
    }
    if config.revision > MAX_SAFE_REVISION || inventory.revision > MAX_SAFE_REVISION {
        return Err("Revision exceeds the desktop's exact integer range (0–9007199254740991). Use the CLI for this snapshot.".into());
    }
    // Desktop inspection work bounds, never product routing capacities. Reject
    // the whole import; the core alone decides policy and backend capacity.
    if config.sources.len() > 32
        || config.mixes.len() > 32
        || config.outputs.len() > 32
        || config.sends.len() > 256
        || config.media_controls.len() > 64
        || inventory.sources.len() > 64
        || inventory.outputs.len() > 64
        || inventory.media_sessions.len() > 128
    {
        return Err("Snapshot exceeds desktop inspection bounds (32 sources/mixes/outputs, 256 sends, 64 policies/source observations/output observations, 128 sessions). Nothing was truncated; use CLI inspect for larger snapshots.".into());
    }
    let response = ImportedInspection {
        schema_version: 1,
        provenance: "user-import",
        freshness: "unverified",
        report: diagnostics::inspect(&config, &inventory),
    };
    let mut reply = BoundedReply(Vec::new());
    serde_json::to_writer(&mut reply, &response)
        .map_err(|error| format!("Inspection reply rejected: {error}"))?;
    String::from_utf8(reply.0).map_err(|error| format!("Inspection encoding failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn inputs() -> (Value, Value) {
        (
            serde_json::from_str(include_str!("../../../../examples/config.json")).unwrap(),
            serde_json::from_str(include_str!("../../../../examples/inventory.json")).unwrap(),
        )
    }
    fn run(config: &Value, inventory: &Value) -> Result<Value, String> {
        inspect(SnapshotRequest {
            config_json: config.to_string(),
            inventory_json: inventory.to_string(),
        })
        .map(|reply| serde_json::from_str(&reply).unwrap())
    }

    #[test]
    fn imported_report_is_exactly_the_authoritative_inspector() {
        let (c, i) = inputs();
        let reply = run(&c, &i).unwrap();
        let config: Config = serde_json::from_value(c).unwrap();
        let inventory: Inventory = serde_json::from_value(i).unwrap();
        assert_eq!(reply["provenance"], "user-import");
        assert_eq!(reply["freshness"], "unverified");
        assert_eq!(
            reply["report"],
            serde_json::to_value(diagnostics::inspect(&config, &inventory)).unwrap()
        );
        assert_eq!(reply["report"]["nativeState"], "unknown");
        assert_eq!(reply["report"]["liveLevels"], Value::Null);
    }

    #[test]
    fn rejection_preserves_core_reasons_and_withholds_every_coefficient() {
        let (mut c, i) = inputs();
        c["outputs"][2]["allowMicMonitor"] = json!(false);
        let reply = run(&c, &i).unwrap();
        assert_eq!(reply["report"]["planAvailable"], false);
        assert_eq!(reply["report"]["plan"], Value::Null);
        let outputs = reply["report"]["outputs"].as_array().unwrap();
        assert!(outputs
            .iter()
            .flat_map(|o| o["paths"].as_array().unwrap())
            .all(|p| p["plannedEffectiveCoefficient"].is_null()));
        let mic = outputs
            .iter()
            .find(|o| o["configured"]["id"] == "headphones")
            .unwrap()["paths"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["source"] == "mic")
            .unwrap();
        assert_eq!(mic["intendedPotentialSharing"], true);
        assert!(mic["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("send-zero")));
        assert!(mic["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("mic-monitor-consent-required")));
    }

    #[test]
    fn malformed_schema_oversized_and_unrepresentable_imports_fail_closed() {
        let (mut c, mut i) = inputs();
        i["schemaVersion"] = json!(1);
        assert!(run(&c, &i).unwrap_err().contains("schema-2"));
        i["schemaVersion"] = json!(2);
        c["revision"] = json!(MAX_SAFE_REVISION + 1);
        assert!(run(&c, &i).unwrap_err().contains("exact integer"));
        c["revision"] = json!(1);
        c["sources"] = json!(vec![c["sources"][0].clone(); 33]);
        assert!(run(&c, &i).unwrap_err().contains("Nothing was truncated"));
        assert!(inspect(SnapshotRequest {
            config_json: " ".repeat(MAX_INPUT_BYTES + 1),
            inventory_json: "{}".into()
        })
        .unwrap_err()
        .contains("128 KiB"));
        assert!(inspect(SnapshotRequest {
            config_json: "/etc/passwd".into(),
            inventory_json: "{}".into()
        })
        .unwrap_err()
        .contains("JSON rejected"));
    }

    #[test]
    fn admission_and_reply_are_bounded() {
        let admission = acquire().unwrap();
        assert!(acquire().is_err());
        drop(admission);
        assert!(acquire().is_ok());
        let mut reply = BoundedReply(Vec::new());
        assert!(reply.write_all(&vec![0; MAX_RESPONSE_BYTES]).is_ok());
        assert!(reply.write_all(&[0]).is_err());
        assert_eq!(reply.0.len(), MAX_RESPONSE_BYTES);
    }
}
