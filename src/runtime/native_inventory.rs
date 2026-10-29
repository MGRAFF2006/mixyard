//! Installed Windows/CoreAudio helpers: fixed `snapshot`, bounded I/O, no audio dispatch.
use super::{
    inventory::{self, Capture, Options, Result},
    inventory_json::Unique,
};
use crate::{Evidence, Inventory, Origin, Platform, Selector, Target};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

pub(super) const HELPER_BUDGET: Duration = Duration::from_millis(700);
pub(super) const MAX_OUTPUT: usize = 1024 * 1024;

pub(super) struct Job {
    child: super::inventory_os::Process,
    output: Vec<u8>,
    started: Instant,
    platform: Platform,
    simulated: bool,
}
impl Job {
    pub(super) fn start(options: &Options, platform: Platform) -> Result<Self> {
        Ok(Self {
            child: super::inventory_os::Process::start(options.fixture_helper.as_deref())?,
            output: Vec::new(),
            started: Instant::now(),
            platform,
            simulated: options.fixture_helper.is_some(),
        })
    }
}
impl Capture for Job {
    fn poll(&mut self) -> Result<Option<Inventory>> {
        if self.started.elapsed() >= HELPER_BUDGET {
            return Err("native-inventory-timeout-700ms");
        }
        if !self.child.poll(&mut self.output)? {
            return Ok(None);
        }
        let inventory = decode(&self.output, self.platform, self.simulated)?;
        if self.started.elapsed() >= HELPER_BUDGET {
            return Err("native-inventory-validation-deadline");
        }
        Ok(Some(inventory))
    }
}

fn decode(bytes: &[u8], platform: Platform, simulated: bool) -> Result<Inventory> {
    if bytes.len() > MAX_OUTPUT || bytes.last() != Some(&b'\n') {
        return Err("native-inventory-truncated-or-output-limit");
    }
    let Unique(value) = serde_json::from_slice(bytes)
        .map_err(|_| "native-inventory-malformed-json-or-duplicate-key")?;
    for key in [
        "schemaVersion",
        "revision",
        "platform",
        "evidence",
        "sources",
        "outputs",
        "mediaSessions",
        "limits",
    ] {
        if value.get(key).is_none() {
            return Err("native-inventory-incomplete-snapshot");
        }
    }
    for key in ["maxExportedMixes", "maxOutputRoutes", "maxVirtualInputs"] {
        if value["limits"].get(key).is_none() {
            return Err("native-inventory-incomplete-limits");
        }
    }
    for key in ["sources", "outputs"] {
        if value[key]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v.get("format").is_none()))
        {
            return Err("native-inventory-incomplete-format");
        }
    }
    let mut inventory: Inventory =
        serde_json::from_value(value).map_err(|_| "native-inventory-malformed-schema")?;
    if inventory.schema_version != 2 || inventory.revision != 0 || inventory.platform != platform {
        return Err("native-inventory-wrong-platform-schema-or-revision");
    }
    // No child can authorize its own evidence, even if it claims native discovery.
    inventory.evidence = if simulated {
        Evidence::Simulated
    } else {
        Evidence::NativeDiscovery
    };
    if !inventory.media_sessions.is_empty()
        || inventory.limits.max_exported_mixes.is_some()
        || inventory.limits.max_output_routes.is_some()
        || inventory.limits.max_virtual_inputs.is_some()
    {
        return Err("native-inventory-unsupported-helper-claims");
    }
    let mut ids = std::collections::BTreeSet::new();
    if inventory
        .sources
        .iter()
        .any(|s| !ids.insert(s.observed_id.as_str()))
    {
        return Err("native-inventory-duplicate-observed-identity");
    }
    ids.clear();
    if inventory
        .outputs
        .iter()
        .any(|s| !ids.insert(s.observed_id.as_str()))
    {
        return Err("native-inventory-duplicate-observed-identity");
    }
    if inventory
        .sources
        .iter()
        .filter_map(|s| s.format)
        .chain(inventory.outputs.iter().filter_map(|s| s.format))
        .any(|f| !matches!(f.channels, 1 | 2) || f.sample_rate_hz == 0)
    {
        return Err("native-inventory-invalid-concrete-format");
    }
    inventory::validate(&inventory)?;
    // Admit exactly the read-only helper's observation domain. Windows process/app
    // identities are deliberately absent: endpoint IDs cannot substitute for them.
    for source in &inventory.sources {
        if !(matches!(&source.selector, Selector::Mic { .. }) && source.origin == Origin::External)
            && !(platform == Platform::Macos
                && matches!(&source.selector, Selector::App { .. })
                && matches!(source.origin, Origin::External | Origin::MixyardProducer)
                && source.format.is_none())
        {
            return Err("native-inventory-unsupported-source-identity");
        }
    }
    if inventory
        .outputs
        .iter()
        .any(|o| !matches!(o.target, Target::Device { .. }))
    {
        return Err("native-inventory-unsupported-output-identity");
    }
    // Helper ordinal observedIds depend on enumeration order. Derive lossless IDs
    // from the exact typed selector/target, keeping duplicates visibly ambiguous.
    // Hash only the observedId; native selector/target strings remain byte-exact.
    for source in &mut inventory.sources {
        source.observed_id = format!(
            "source:{:x}",
            Sha256::digest(
                serde_json::to_vec(&source.selector).map_err(|_| "native-inventory-identity")?
            )
        );
    }
    for output in &mut inventory.outputs {
        output.observed_id = format!(
            "output:{:x}",
            Sha256::digest(
                serde_json::to_vec(&output.target).map_err(|_| "native-inventory-identity")?
            )
        );
    }
    // Include all represented properties when ordering duplicate identities.
    inventory
        .sources
        .sort_by_cached_key(|s| serde_json::to_string(s).unwrap());
    inventory
        .outputs
        .sort_by_cached_key(|o| serde_json::to_string(o).unwrap());
    // Identical IDs still need distinct occurrence IDs for schema admission.
    for (i, source) in inventory.sources.iter_mut().enumerate() {
        source.observed_id = format!("{i}:{}", source.observed_id);
    }
    for (i, output) in inventory.outputs.iter_mut().enumerate() {
        output.observed_id = format!("{i}:{}", output.observed_id);
    }
    inventory::validate(&inventory)?;
    Ok(inventory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    fn snapshot() -> Value {
        json!({"schemaVersion":2,"revision":0,"platform":"windows","evidence":"native-discovery",
            "sources":[{"observedId":"windows:source:0","selector":{"kind":"mic","deviceUid":"{exact-endpoint-9007199254740993}"},"origin":"external","format":{"channels":2,"sampleRateHz":44100}}],
            "outputs":[{"observedId":"windows:output:0","target":{"kind":"device","deviceUid":"output-Ü"},"format":null}],
            "mediaSessions":[],"limits":{"maxExportedMixes":null,"maxOutputRoutes":null,"maxVirtualInputs":null}})
    }
    fn parse(value: &Value) -> Result<Inventory> {
        let mut bytes = serde_json::to_vec(value).unwrap();
        bytes.push(b'\n');
        decode(&bytes, Platform::Windows, true)
    }
    #[test]
    fn fixture_claims_cannot_become_native_and_exact_ids_and_unknown_formats_survive() {
        let inventory = parse(&snapshot()).unwrap();
        assert_eq!(inventory.evidence, Evidence::Simulated);
        assert_eq!(
            inventory.sources[0].selector,
            Selector::Mic {
                device_uid: "{exact-endpoint-9007199254740993}".into()
            }
        );
        assert_eq!(inventory.sources[0].format.unwrap().sample_rate_hz, 44100);
        assert_eq!(inventory.outputs[0].format, None);
        let mut large = snapshot();
        large["sources"][0]["selector"]["deviceUid"] = json!("x".repeat(4096));
        assert!(parse(&large).is_ok());
        large["sources"][0]["selector"]["deviceUid"] = json!("x".repeat(4097));
        assert!(parse(&large).is_err());
    }
    #[test]
    fn enumeration_order_is_normalized_and_duplicate_native_selectors_stay_ambiguous() {
        let mut a = snapshot();
        let mut second = a["sources"][0].clone();
        second["observedId"] = json!("windows:source:1");
        second["selector"]["deviceUid"] = json!("other");
        a["sources"].as_array_mut().unwrap().push(second);
        let first = parse(&a).unwrap();
        a["sources"].as_array_mut().unwrap().reverse();
        for (i, source) in a["sources"].as_array_mut().unwrap().iter_mut().enumerate() {
            source["observedId"] = json!(format!("windows:source:{i}"));
        }
        assert_eq!(
            serde_json::to_value(first).unwrap(),
            serde_json::to_value(parse(&a).unwrap()).unwrap()
        );
        a["sources"][1]["selector"] = a["sources"][0]["selector"].clone();
        let duplicated = parse(&a).unwrap();
        assert_eq!(duplicated.sources.len(), 2);
        assert_eq!(
            duplicated.sources[0].selector,
            duplicated.sources[1].selector
        );
        assert_ne!(
            duplicated.sources[0].observed_id,
            duplicated.sources[1].observed_id
        );
        a["sources"][1]["observedId"] = a["sources"][0]["observedId"].clone();
        assert!(parse(&a).is_err());
    }
    #[test]
    fn malformed_partial_wrong_os_and_unsupported_claims_never_become_empty_success() {
        for (field, value) in [
            ("platform", json!("macos")),
            ("revision", json!(1)),
            ("schemaVersion", json!(1)),
            ("extra", json!(true)),
        ] {
            let mut value_snapshot = snapshot();
            value_snapshot[field] = value;
            assert!(parse(&value_snapshot).is_err(), "{field}");
        }
        for field in ["sources", "outputs", "mediaSessions", "limits"] {
            let mut v = snapshot();
            v.as_object_mut().unwrap().remove(field);
            assert!(parse(&v).is_err(), "missing {field}");
        }
        let mut v = snapshot();
        v["sources"][0].as_object_mut().unwrap().remove("format");
        assert!(parse(&v).is_err());
        v = snapshot();
        v["sources"][0]["format"]["channels"] = json!(8);
        assert!(parse(&v).is_err());
        v = snapshot();
        v["sources"][0]["selector"] = json!({"kind":"app","applicationId":"guessed.exe"});
        assert!(
            parse(&v).is_err(),
            "Windows app identities are not endpoint identities"
        );
        let raw = serde_json::to_string(&snapshot()).unwrap();
        assert!(
            decode(raw.as_bytes(), Platform::Windows, true).is_err(),
            "missing terminal LF"
        );
        let duplicate = raw.replacen("\"revision\":0", "\"revision\":0,\"revision\":0", 1) + "\n";
        assert!(decode(duplicate.as_bytes(), Platform::Windows, true).is_err());
        assert!(decode(&vec![b' '; MAX_OUTPUT + 1], Platform::Windows, true).is_err());
        v = snapshot();
        let mut entries = Vec::new();
        for i in 0..1025 {
            let mut s = v["sources"][0].clone();
            s["observedId"] = json!(i.to_string());
            entries.push(s);
        }
        v["sources"] = json!(entries);
        assert!(parse(&v).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn actual_posix_helper_subprocess_is_bounded_and_never_forwards_private_errors() {
        use std::{fs, os::unix::fs::PermissionsExt, thread};
        let directory =
            std::env::temp_dir().join(format!("mixyard-native-process-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let helper = directory.join("helper");
        let script = |body: &str| {
            fs::write(&helper, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        };
        let options = Options {
            fixture_helper: Some(helper.clone()),
        };
        let run = || {
            let mut job = Job::start(&options, Platform::Windows).unwrap();
            loop {
                match job.poll() {
                    Ok(None) => thread::sleep(Duration::from_millis(5)),
                    result => return result,
                }
            }
        };
        script(&format!("printf '%s\\n' '{}'", snapshot()));
        assert_eq!(run().unwrap().unwrap().evidence, Evidence::Simulated);
        for body in [
            "echo private-selector >&2; exit 1",
            "printf '{'",
            "sleep 30 & wait",
        ] {
            script(body);
            let start = Instant::now();
            let error = run().unwrap_err();
            assert!(!error.contains("private-selector"));
            assert!(start.elapsed() < Duration::from_secs(2));
        }
        fs::remove_dir_all(directory).unwrap();
    }
}
