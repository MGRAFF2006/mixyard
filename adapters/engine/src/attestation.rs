//! Bounded control metadata, never authentication or native provenance evidence.
use super::*;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_ENTRIES: usize = 4096;
const MAX_JSON: usize = 1024 * 1024;
const MAX_CACHE: usize = 4 * MAX_JSON;
static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);

pub(super) struct Cached {
    pub json: Vec<u8>,
    pub digest: [u8; 32],
}
struct Bounded {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|n| n > self.limit)
        {
            return Err(io::Error::other("compiled metadata byte limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn json<T: Serialize>(value: &T, limit: usize) -> Option<Vec<u8>> {
    let mut out = Bounded {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut out, value).ok()?;
    Some(out.bytes)
}
pub(super) fn digest(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in hash {
        use std::fmt::Write;
        write!(out, "{byte:02x}").expect("String formatting");
    }
    out
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Contributor<'a> {
    source_index: usize,
    source_id: &'a str,
    selector: &'a mixyard::Selector,
    origin: mixyard::Origin,
    binding: &'a SourceSpec,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Metadata<'a> {
    schema_version: u32,
    scope: &'static str,
    complete: bool,
    process_id: u32,
    instance_id: u64,
    generation: u64,
    revision: u64,
    inventory_revision: u64,
    clock_id: u64,
    request_sha256: &'a str,
    plan_sha256: &'a str,
    dsp_snapshot_sha256: &'a str,
    output_index: usize,
    output_id: &'a str,
    mix_id: &'a str,
    output: &'a OutputSpec,
    contributors: Vec<Contributor<'a>>,
}
type DspSnapshot<'a> = (
    &'a Vec<Option<dsp::ChainConfig>>,
    &'a Vec<Option<dsp::ChainConfig>>,
    &'a Vec<Option<dsp::ChainConfig>>,
);
pub(super) struct Input<'a> {
    pub plan: &'a mixyard::Plan,
    pub inventory: &'a Inventory,
    pub sources: &'a [Option<SourceSpec>],
    pub outputs: &'a [Option<OutputSpec>],
    pub dsp: DspSnapshot<'a>,
    pub generation: u64,
    pub clock_id: u64,
    pub request_digest: String,
}
pub(super) fn compile(input: Input<'_>) -> Vec<Option<Cached>> {
    let Input {
        plan,
        inventory,
        sources,
        outputs,
        dsp,
        generation,
        clock_id,
        request_digest,
    } = input;
    let mut result: Vec<_> = (0..outputs.len()).map(|_| None).collect();
    if sources.len() > MAX_ENTRIES
        || outputs.len() > MAX_ENTRIES
        || plan.sends.len() > MAX_ENTRIES
        || inventory.sources.len() > MAX_ENTRIES
    {
        return result;
    }
    let Some(plan_json) = json(plan, MAX_JSON) else {
        return result;
    };
    let Some(dsp_json) = json(&dsp, MAX_JSON) else {
        return result;
    };
    let plan_digest = digest(&plan_json);
    let dsp_digest = digest(&dsp_json);
    let Ok(instance_id) =
        NEXT_INSTANCE.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
    else {
        return result;
    };
    let source_slots: BTreeMap<_, _> = plan
        .sources
        .iter()
        .enumerate()
        .map(|(i, s)| (s.source.as_str(), i))
        .collect();
    let observations: BTreeMap<_, _> = inventory
        .sources
        .iter()
        .map(|s| (s.observed_id.as_str(), s))
        .collect();
    let mut remaining = MAX_CACHE;
    for (index, output) in plan.outputs.iter().enumerate() {
        let Some(Some(binding)) = outputs.get(index) else {
            continue;
        };
        if output.status != ResolutionStatus::Selected
            || output.observed_ids.as_slice() != [binding.observed_id.as_str()]
        {
            continue;
        }
        let mut contributors = Vec::new();
        let mut complete = true;
        // Exact dependency edges from the canonical plan. No gain/mute/sample
        // predicate: all reachable sources remain in the union for its lifetime.
        // Index once instead of a source-by-send cubic walk over all outputs.
        let mut reachable = vec![false; sources.len()];
        for send in plan.sends.iter().filter(|send| send.mix == output.mix) {
            let Some(&slot) = source_slots.get(send.source.as_str()) else {
                complete = false;
                break;
            };
            reachable[slot] = true;
        }
        if !complete {
            continue;
        }
        for (source_index, source) in plan.sources.iter().enumerate() {
            if !reachable[source_index] {
                continue;
            }
            let Some(Some(spec)) = sources.get(source_index) else {
                complete = false;
                break;
            };
            let Some(observation) = observations.get(spec.observed_id.as_str()) else {
                complete = false;
                break;
            };
            if source.status != ResolutionStatus::Selected
                || source.observed_ids.as_slice() != [spec.observed_id.as_str()]
                || observation.selector != source.selector
            {
                complete = false;
                break;
            }
            // Cardinalities/serialization limits above bound every borrowed
            // entry before serialization. No label/binding is cloned.
            contributors.push(Contributor {
                source_index,
                source_id: &source.source,
                selector: &source.selector,
                origin: observation.origin,
                binding: spec,
            });
        }
        if !complete {
            continue;
        }
        let metadata = Metadata {
            schema_version: 1,
            scope: "compiled-plan-metadata",
            complete: true,
            process_id: std::process::id(),
            instance_id,
            generation,
            revision: plan.config_revision,
            inventory_revision: plan.inventory_revision,
            clock_id,
            request_sha256: &request_digest,
            plan_sha256: &plan_digest,
            dsp_snapshot_sha256: &dsp_digest,
            output_index: index,
            output_id: &output.id,
            mix_id: &output.mix,
            output: binding,
            contributors,
        };
        if let Some(bytes) = json(&metadata, remaining.min(MAX_JSON)) {
            remaining -= bytes.len();
            result[index] = Some(Cached {
                digest: Sha256::digest(&bytes).into(),
                json: bytes,
            });
        }
    }
    result
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MxOutputPacket {
    pub samples: *const f64,
    pub sample_count: usize,
    pub stamp: MxStamp,
    pub output_index: u64,
    pub attestation_sha256: [u8; 32],
}
impl MxOutputPacket {
    fn empty() -> Self {
        Self {
            samples: ptr::null(),
            sample_count: 0,
            stamp: MxStamp::default(),
            output_index: 0,
            attestation_sha256: [0; 32],
        }
    }
}
/// # Safety
/// Live context, exclusive control owner, valid disjoint needed/byte buffers.
/// No concurrent process/revoke/destroy. Metadata is not native authorization.
#[no_mangle]
pub unsafe extern "C" fn mx_host_output_attestation_v1(
    host: *const MxHost,
    index: usize,
    bytes: *mut u8,
    capacity: usize,
    needed: *mut usize,
) -> i32 {
    if needed.is_null() {
        return MX_INVALID;
    }
    *needed = 0;
    let Some(host) = host.as_ref() else {
        return MX_INVALID;
    };
    if !host.active {
        return MX_INACTIVE;
    }
    let Some(value) = host.attestations.get(index) else {
        return MX_INVALID;
    };
    let Some(value) = value else {
        return MX_UNAVAILABLE;
    };
    *needed = value.json.len();
    if bytes.is_null() || capacity < value.json.len() {
        return MX_BUFFER;
    }
    ptr::copy_nonoverlapping(value.json.as_ptr(), bytes, value.json.len());
    MX_OK
}
/// # Safety
/// Same exclusive, live-context and borrowing requirements as mx_host_output.
/// Packet is valid, disjoint storage. Check external leases again at commit.
#[no_mangle]
pub unsafe extern "C" fn mx_host_output_packet_v1(
    host: *const MxHost,
    index: usize,
    packet: *mut MxOutputPacket,
) -> i32 {
    let Some(packet) = packet.as_mut() else {
        return MX_INVALID;
    };
    *packet = MxOutputPacket::empty();
    let Some(host) = host.as_ref() else {
        return MX_INVALID;
    };
    if !host.active {
        return MX_INACTIVE;
    }
    let Some(cached) = host.attestations.get(index) else {
        return MX_INVALID;
    };
    let Some(cached) = cached else {
        return MX_UNAVAILABLE;
    };
    let Some(stamp) = host.output_stamp else {
        return MX_INACTIVE;
    };
    let Some(Some(output)) = host.outputs.get(index) else {
        return MX_UNAVAILABLE;
    };
    if !output.active || output.count == 0 {
        return MX_INACTIVE;
    }
    *packet = MxOutputPacket {
        samples: output.samples.as_ptr(),
        sample_count: output.count,
        stamp,
        output_index: index as u64,
        attestation_sha256: cached.digest,
    };
    MX_OK
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_metadata_never_returns_truncated_json() {
        let value = vec!["long escaped \" label"; 8];
        let full = serde_json::to_vec(&value).unwrap();
        assert!(json(&value, full.len() - 1).is_none());
        assert_eq!(json(&value, full.len()).unwrap(), full);
        let mut out = Bounded {
            bytes: Vec::new(),
            limit: 3,
        };
        assert_eq!(out.write(b"abc").unwrap(), 3);
        assert!(out.write(b"d").is_err());
        assert_eq!(out.bytes, b"abc");
    }
}
