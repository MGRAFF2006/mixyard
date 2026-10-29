use super::*;
use serde_json::{json, Value};
use sha2::Digest;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Tracked;
thread_local! { static HEAP: Cell<Option<usize>> = const { Cell::new(None) }; }
fn record() {
    let _ = HEAP.try_with(|n| {
        if let Some(v) = n.get() {
            n.set(Some(v + 1));
        }
    });
}
unsafe impl GlobalAlloc for Tracked {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        record();
        System.alloc(l)
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        record();
        System.alloc_zeroed(l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        record();
        System.realloc(p, l, n)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        record();
        System.dealloc(p, l)
    }
}
#[global_allocator]
static ALLOCATOR: Tracked = Tracked;

fn fixture() -> Value {
    serde_json::from_str(include_str!("../fixtures/owned-common-clock.json")).unwrap()
}
struct Owned(*mut MxHost);
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe {
            mx_host_destroy(self.0);
        }
    }
}
fn create(value: Value) -> Owned {
    let bytes = serde_json::to_vec(&value).unwrap();
    let mut host = ptr::null_mut();
    let mut error = [0i8; 4096];
    let status = unsafe {
        mx_host_create(
            bytes.as_ptr(),
            bytes.len(),
            &mut host,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    assert_eq!(
        status,
        MX_OK,
        "{}",
        unsafe { std::ffi::CStr::from_ptr(error.as_ptr()) }.to_string_lossy()
    );
    Owned(host)
}
fn rejected(value: Value) {
    let bytes = serde_json::to_vec(&value).unwrap();
    let mut host = ptr::null_mut();
    let mut error = [0i8; 4096];
    assert_ne!(
        unsafe {
            mx_host_create(
                bytes.as_ptr(),
                bytes.len(),
                &mut host,
                error.as_mut_ptr(),
                error.len(),
            )
        },
        MX_OK
    );
    assert!(host.is_null());
    assert_ne!(error[0], 0);
}
fn stamp(sequence: u64) -> MxStamp {
    let now = 100 + sequence * 50_000;
    MxStamp {
        generation: 7,
        revision: 1,
        clock_id: 11,
        sequence,
        first_frame: sequence * 2,
        captured_ns: now,
        deadline_ns: now + 1_000_000,
    }
}
fn bytes(values: &[f64]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}
fn blocks() -> [Vec<u8>; 3] {
    [
        bytes(&[0.9; 4]),
        bytes(&[0.2, -0.2, 0.4, -0.4]),
        bytes(&[0.4, -0.4, 0.8, -0.8]),
    ]
}
fn inputs(data: &[Vec<u8>; 3], stamp: MxStamp) -> [MxInput; 3] {
    std::array::from_fn(|i| MxInput {
        bytes: data[i].as_ptr(),
        byte_count: data[i].len(),
        encoding: 2,
        sample_rate_hz: 48_000,
        channels: 2,
        stamp,
    })
}
fn run(host: &Owned, sequence: u64, input: &[MxInput; 3]) -> i32 {
    let s = stamp(sequence);
    unsafe { mx_host_process(host.0, s.captured_ns, s, 2, input.as_ptr(), input.len()) }
}
fn output(host: &Owned, index: usize) -> Vec<f64> {
    let mut pointer = ptr::null();
    let mut count = 0;
    assert_eq!(
        unsafe { mx_host_output(host.0, index, &mut pointer, &mut count) },
        MX_OK
    );
    unsafe { slice::from_raw_parts(pointer, count).to_vec() }
}
fn inactive(host: &Owned, index: usize) {
    let mut pointer = std::ptr::dangling::<f64>();
    let mut count = 99;
    assert_eq!(
        unsafe { mx_host_output(host.0, index, &mut pointer, &mut count) },
        MX_INACTIVE
    );
    assert!(pointer.is_null());
    assert_eq!(count, 0);
}

#[test]
fn actual_ffi_port_contract_private_exclusion_mix_minus_and_output_dsp() {
    let value = fixture();
    let host = create(value.clone());
    assert_eq!(unsafe { mx_host_source_count(host.0) }, 3);
    assert_eq!(unsafe { mx_host_output_count(host.0) }, 2);
    let mut needed = 0;
    assert_eq!(
        unsafe { mx_host_source_info(host.0, 0, ptr::null_mut(), 0, &mut needed) },
        MX_BUFFER
    );
    let mut buffer = vec![0; needed];
    assert_eq!(
        unsafe { mx_host_source_info(host.0, 0, buffer.as_mut_ptr(), buffer.len(), &mut needed) },
        MX_OK
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&buffer).unwrap(),
        value["sources"][0]
    );
    assert_eq!(
        String::from_utf8(buffer).unwrap(),
        r#"{"observedId":"owned-source-private","encoding":"f64-le","sampleRateHz":48000,"channels":["FL","FR"],"mapping":["FL","FR"]}"#
    );
    let data = blocks();
    let input = inputs(&data, stamp(0));
    assert_eq!(run(&host, 0, &input), MX_OK);
    assert_eq!(output(&host, 0), [0.4, -0.4, 0.8, -0.8]);
    assert_eq!(output(&host, 1), [0.125, -0.125, 0.25, -0.25]);
}

#[test]
fn native_integer_and_float_ingress_retain_precision_without_float32_intermediate() {
    let cases: &[(&str, u32, &[u8], f64)] = &[
        ("s16-le", 3, &[1, 0x40], 0.5 + 2f64.powi(-15)),
        ("s24-le", 4, &[1, 0, 0x40], 0.5 + 2f64.powi(-23)),
        ("s32-le", 5, &[1, 0, 0, 0x40], 0.5 + 2f64.powi(-31)),
        ("f32-le", 1, &[1, 0, 0, 0x3f], 0.5 + 2f64.powi(-24)),
    ];
    for &(encoding, code, packed, expected) in cases {
        let mut value = fixture();
        value["sources"][2]["encoding"] = json!(encoding);
        let host = create(value);
        let mut data = blocks();
        data[2] = packed.repeat(4);
        let mut input = inputs(&data, stamp(0));
        input[2].encoding = code;
        assert_eq!(run(&host, 0, &input), MX_OK);
        assert_eq!(
            output(&host, 0)
                .iter()
                .map(|v| v.to_bits())
                .collect::<Vec<_>>(),
            vec![expected.to_bits(); 4]
        );
    }
    let host = create(fixture());
    let mut data = blocks();
    let expected = [0.5 + 2f64.powi(-40), -0.0, f64::from_bits(1), -0.5];
    data[2] = bytes(&expected);
    assert_eq!(run(&host, 0, &inputs(&data, stamp(0))), MX_OK);
    assert_eq!(
        output(&host, 0)
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>(),
        expected.map(f64::to_bits)
    );
}

#[test]
fn source_mix_output_processing_use_one_canonical_graph_and_keep_destination_failures_local() {
    let mut value = fixture();
    let mut chain = value["outputDsp"][1].clone();
    chain["effects"][0]["effect"]["gain"] = json!(2.0);
    value["sourceDsp"][2] = chain.clone();
    chain["effects"][0]["effect"]["gain"] = json!(0.5);
    value["mixDsp"][0] = chain.clone();
    value["outputDsp"][0] = chain;
    let host = create(value);
    let mut data = blocks();
    data[1] = bytes(&[0.25; 4]);
    data[2] = bytes(&[0.25; 4]);
    assert_eq!(run(&host, 0, &inputs(&data, stamp(0))), MX_OK);
    assert_eq!(output(&host, 0), [0.125; 4]);
    assert_eq!(output(&host, 1), [0.15625; 4]);
    let mut value = fixture();
    value["outputDsp"][1]["effects"][0]["effect"]["gain"] = json!(f64::MAX);
    let host = create(value);
    data[1] = bytes(&[4.0; 4]);
    data[2] = bytes(&[2.0; 4]);
    assert_eq!(run(&host, 0, &inputs(&data, stamp(0))), MX_OK);
    assert_eq!(output(&host, 0), [2.0; 4]);
    inactive(&host, 1);
    data[1] = bytes(&[0.2; 4]);
    data[2] = bytes(&[0.5; 4]);
    assert_eq!(run(&host, 1, &inputs(&data, stamp(1))), MX_OK);
    assert_eq!(output(&host, 0), [0.5; 4]);
    inactive(&host, 1);
}

#[test]
fn missing_source_is_silent_and_cannot_resume_without_new_admission() {
    let host = create(fixture());
    let data = blocks();
    let mut input = inputs(&data, stamp(0));
    input[1].bytes = ptr::null();
    input[1].byte_count = 0;
    assert_eq!(run(&host, 0, &input), MX_OK);
    assert_eq!(output(&host, 0), [0.4, -0.4, 0.8, -0.8]);
    assert_eq!(output(&host, 1), [0.1, -0.1, 0.2, -0.2]);
    assert_eq!(run(&host, 1, &inputs(&data, stamp(1))), MX_STALE);
    inactive(&host, 0);
    inactive(&host, 1);
}

#[test]
fn source_loss_discards_derived_mix_and_output_effect_tails() {
    let mut value = fixture();
    let mut delay = value["outputDsp"][1].clone();
    delay["effects"][0]["effect"] =
        json!({"kind":"delay","frames":2,"wet":1.0,"dry":0.0,"feedback":0.0});
    value["mixDsp"][0] = delay.clone();
    value["outputDsp"][0] = delay;
    let host = create(value);
    let data = blocks();
    assert_eq!(run(&host, 0, &inputs(&data, stamp(0))), MX_OK);
    for sequence in 1..4 {
        let mut input = inputs(&data, stamp(sequence));
        input[2].bytes = ptr::null();
        input[2].byte_count = 0;
        HEAP.with(|n| n.set(Some(0)));
        assert_eq!(run(&host, sequence, &input), MX_OK);
        let operations = HEAP.with(|n| n.replace(None).unwrap());
        assert_eq!(
            operations, 0,
            "derived history reset must remain callback-safe"
        );
        assert_eq!(
            output(&host, 0),
            [0.0; 4],
            "a lost source must not reappear through retained derived effects"
        );
        assert_eq!(output(&host, 1), [0.025, -0.025, 0.05, -0.05]);
    }
}

#[test]
fn malformed_stale_expired_or_regressing_data_clears_all_retained_audio() {
    for mode in 0..8 {
        let host = create(fixture());
        let mut data = blocks();
        assert_eq!(run(&host, 0, &inputs(&data, stamp(0))), MX_OK);
        if mode == 4 {
            data[0] = bytes(&[f64::NAN; 4]);
        } // Even private/unrouted input validates.
        let mut input = inputs(&data, stamp(1));
        match mode {
            0 => input[2].encoding = 1,
            1 => input[2].byte_count -= 1,
            2 => input[2].stamp.generation += 1,
            3 => input[2].stamp.deadline_ns = 0,
            5 => input[2].stamp.captured_ns = 99,
            6 => input[2].sample_rate_hz = 44_100,
            7 => input[2].channels = 1,
            _ => {}
        }
        assert_ne!(run(&host, 1, &input), MX_OK);
        inactive(&host, 0);
        inactive(&host, 1);
        assert_eq!(run(&host, 2, &inputs(&data, stamp(2))), MX_INACTIVE);
    }
}

#[test]
fn policy_endpoint_format_budget_and_consent_reject_before_any_native_activation() {
    for mode in 0..6 {
        let mut value = fixture();
        match mode {
            0 => value["sources"][2]["observedId"] = json!("different-actual-endpoint"),
            1 => value["inventory"]["sources"][2]["origin"] = json!("mixyard-producer"),
            2 => value["sources"][2]["mapping"] = json!(["FR", "FL"]),
            3 => value["sources"][2]["channels"] = json!(["FL", "FL"]),
            4 => value["limits"]["maxTotalBytes"] = json!(1),
            _ => value["outputDsp"][1]["format"]["sampleRateHz"] = json!(44_100),
        }
        rejected(value);
    }
    let mut value = fixture();
    let mic = json!({"kind":"mic","deviceUid":"owned-synthetic-input"});
    value["config"]["sources"][0]["selector"] = mic.clone();
    value["inventory"]["sources"][0]["selector"] = mic;
    value["config"]["sends"][0]["gain"] = json!(0.5);
    value["config"]["sends"]
        .as_array_mut()
        .unwrap()
        .push(json!({"source":"private","mix":"call","gain":0.1}));
    value["limits"]["maxSends"] = json!(4);
    let target = json!({"kind":"device","deviceUid":"owned-null-output"});
    value["config"]["outputs"][0]["target"] = target.clone();
    value["inventory"]["outputs"][0]["target"] = target;
    rejected(value.clone());
    value["config"]["outputs"][0]["allowMicMonitor"] = json!(true);
    drop(create(value)); // Numeric consent comes from the actual caller, not invented OS authority.
}

#[test]
fn callback_success_failure_and_revoke_have_no_heap_operations() {
    let host = create(fixture());
    let data = blocks();
    let mut input = inputs(&data, stamp(0));
    HEAP.with(|n| n.set(Some(0)));
    for sequence in 0..512 {
        let s = stamp(sequence);
        for i in &mut input {
            i.stamp = s;
        }
        assert_eq!(run(&host, sequence, &input), MX_OK);
        let mut p = ptr::null();
        let mut count = 0;
        assert_eq!(
            unsafe { mx_host_output(host.0, 0, &mut p, &mut count) },
            MX_OK
        );
        assert_eq!(count, 4);
        assert_eq!(unsafe { *p }, 0.4);
    }
    let missing = stamp(512);
    for i in &mut input {
        i.stamp = missing;
    }
    input[1].bytes = ptr::null();
    input[1].byte_count = 0;
    assert_eq!(run(&host, 512, &input), MX_OK);
    let mut s = stamp(513);
    s.revision += 1;
    assert_eq!(
        unsafe { mx_host_process(host.0, s.captured_ns, s, 2, input.as_ptr(), 3) },
        MX_STALE
    );
    unsafe {
        mx_host_revoke(host.0);
    }
    let operations = HEAP.with(|n| n.replace(None).unwrap());
    assert_eq!(operations, 0);
    inactive(&host, 0);
}

fn attested(host: &Owned, index: usize) -> (Vec<u8>, Value) {
    let mut needed = 0;
    assert_eq!(
        unsafe {
            attestation::mx_host_output_attestation_v1(
                host.0,
                index,
                ptr::null_mut(),
                0,
                &mut needed,
            )
        },
        MX_BUFFER
    );
    let mut bytes = vec![0; needed];
    assert_eq!(
        unsafe {
            attestation::mx_host_output_attestation_v1(
                host.0,
                index,
                bytes.as_mut_ptr(),
                bytes.len(),
                &mut needed,
            )
        },
        MX_OK
    );
    let value = serde_json::from_slice(&bytes).unwrap();
    (bytes, value)
}
fn contributor_ids(value: &Value) -> Vec<&str> {
    value["contributors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["binding"]["observedId"].as_str().unwrap())
        .collect()
}
#[test]
fn per_output_union_matches_actual_call_record_policy_and_pcm() {
    let request = fixture();
    let request_bytes = serde_json::to_vec(&request).unwrap();
    let host = create(request);
    let (_, call) = attested(&host, 0);
    let (_, record) = attested(&host, 1);
    assert_eq!(contributor_ids(&call), ["owned-source-shared"]);
    assert_eq!(
        contributor_ids(&record),
        ["owned-source-return", "owned-source-shared"]
    );
    assert_eq!(call["requestSha256"], attestation::digest(&request_bytes));
    assert_eq!(call["scope"], "compiled-plan-metadata");
    assert_eq!(call["generation"], 7);
    assert_eq!(call["revision"], 1);
    assert_eq!(call["inventoryRevision"], 1);
    assert_eq!(call["clockId"], 11);
    assert_eq!(call["output"]["observedId"], "owned-consumer-call");
    assert_eq!(call["contributors"][0]["sourceIndex"], 2);
    assert_eq!(call["contributors"][0]["origin"], "external");
    assert_eq!(call["planSha256"], record["planSha256"]);
    assert_eq!(call["dspSnapshotSha256"], record["dspSnapshotSha256"]);
    let data = blocks();
    assert_eq!(run(&host, 0, &inputs(&data, stamp(0))), MX_OK);
    assert_eq!(output(&host, 0), [0.4, -0.4, 0.8, -0.8]);
    assert_eq!(output(&host, 1), [0.125, -0.125, 0.25, -0.25]);
}
#[test]
fn muted_and_zero_gain_dependencies_are_never_removed_from_union() {
    let mut value = fixture();
    value["config"]["sends"][0]["mute"] = json!(true);
    value["config"]["sends"][0]["gain"] = json!(0);
    value["config"]["sends"][2]["gain"] = json!(0);
    let host = create(value);
    assert_eq!(
        contributor_ids(&attested(&host, 0).1),
        ["owned-source-shared"]
    );
    assert_eq!(
        contributor_ids(&attested(&host, 1).1),
        ["owned-source-return", "owned-source-shared"]
    );
    let data = blocks();
    assert_eq!(run(&host, 0, &inputs(&data, stamp(0))), MX_OK);
    assert_eq!(output(&host, 0), [0.0; 4]);
}
#[test]
fn unknown_dependency_is_unavailable_but_unreachable_missing_source_is_irrelevant() {
    let mut value = fixture();
    value["inventory"]["sources"]
        .as_array_mut()
        .unwrap()
        .remove(2);
    value["sources"][2] = Value::Null;
    let host = create(value);
    let mut needed = 99;
    let mut untouched = [77u8; 64];
    assert_eq!(
        unsafe {
            attestation::mx_host_output_attestation_v1(
                host.0,
                0,
                untouched.as_mut_ptr(),
                64,
                &mut needed,
            )
        },
        MX_UNAVAILABLE
    );
    assert_eq!(needed, 0);
    assert_eq!(untouched, [77; 64]);
    let mut value = fixture();
    value["inventory"]["sources"]
        .as_array_mut()
        .unwrap()
        .remove(0);
    value["sources"][0] = Value::Null;
    let host = create(value);
    assert_eq!(
        contributor_ids(&attested(&host, 0).1),
        ["owned-source-shared"]
    );
}
#[test]
fn packet_binds_instance_output_json_and_exact_engine_stamp_and_errors_clear_it() {
    let host = create(fixture());
    let other = create(fixture());
    let (json, metadata) = attested(&host, 0);
    let (_, other_meta) = attested(&other, 0);
    assert_ne!(metadata["instanceId"], other_meta["instanceId"]);
    let mut packet = attestation::MxOutputPacket {
        samples: std::ptr::dangling(),
        sample_count: 99,
        stamp: stamp(99),
        output_index: 99,
        attestation_sha256: [88; 32],
    };
    assert_eq!(
        unsafe { attestation::mx_host_output_packet_v1(host.0, 0, &mut packet) },
        MX_INACTIVE
    );
    assert!(packet.samples.is_null());
    assert_eq!(packet.sample_count, 0);
    assert_eq!(packet.stamp.generation, 0);
    assert_eq!(packet.attestation_sha256, [0; 32]);
    let data = blocks();
    assert_eq!(run(&host, 0, &inputs(&data, stamp(0))), MX_OK);
    HEAP.with(|n| n.set(Some(0)));
    for _ in 0..512 {
        assert_eq!(
            unsafe { attestation::mx_host_output_packet_v1(host.0, 0, &mut packet) },
            MX_OK
        );
    }
    let operations = HEAP.with(|n| n.replace(None).unwrap());
    assert_eq!(operations, 0);
    assert_eq!(packet.output_index, 0);
    assert_eq!(packet.sample_count, 4);
    assert_eq!(
        packet.attestation_sha256,
        <[u8; 32]>::from(sha2::Sha256::digest(&json))
    );
    assert_eq!(packet.stamp.clock_id, 11);
    assert_eq!(packet.stamp.sequence, 0);
    assert_eq!(packet.stamp.first_frame, 0);
    assert_eq!(packet.stamp.captured_ns, stamp(0).captured_ns);
    assert_eq!(packet.stamp.deadline_ns, stamp(0).deadline_ns);
    assert_eq!(
        unsafe { slice::from_raw_parts(packet.samples, packet.sample_count) },
        [0.4, -0.4, 0.8, -0.8]
    );
    assert_eq!(
        unsafe { attestation::mx_host_output_packet_v1(host.0, 1, &mut packet) },
        MX_OK
    );
    assert_eq!(packet.output_index, 1);
    assert_ne!(
        packet.attestation_sha256,
        <[u8; 32]>::from(sha2::Sha256::digest(&json))
    );
    unsafe { mx_host_revoke(host.0) };
    assert_eq!(
        unsafe { attestation::mx_host_output_packet_v1(host.0, 0, &mut packet) },
        MX_INACTIVE
    );
    assert!(packet.samples.is_null());
    assert_eq!(packet.sample_count, 0);
    assert_eq!(packet.attestation_sha256, [0; 32]);
    let mut needed = 99;
    assert_eq!(
        unsafe {
            attestation::mx_host_output_attestation_v1(host.0, 0, ptr::null_mut(), 0, &mut needed)
        },
        MX_INACTIVE
    );
    assert_eq!(needed, 0);
}
#[test]
fn query_short_buffers_never_truncate_and_snapshot_digests_change_independently() {
    let host = create(fixture());
    let (full, initial) = attested(&host, 0);
    let mut bytes = vec![99; full.len() - 1];
    let mut needed = 0;
    assert_eq!(
        unsafe {
            attestation::mx_host_output_attestation_v1(
                host.0,
                0,
                bytes.as_mut_ptr(),
                bytes.len(),
                &mut needed,
            )
        },
        MX_BUFFER
    );
    assert_eq!(needed, full.len());
    assert!(bytes.iter().all(|v| *v == 99));
    let mut value = fixture();
    value["outputDsp"][1]["effects"][0]["effect"]["gain"] = json!(0.25);
    let changed = create(value);
    let (_, changed) = attested(&changed, 0);
    assert_ne!(initial["requestSha256"], changed["requestSha256"]);
    assert_ne!(initial["dspSnapshotSha256"], changed["dspSnapshotSha256"]);
    assert_eq!(initial["planSha256"], changed["planSha256"]);
}

#[test]
fn destination_failure_never_borrows_another_outputs_packet() {
    let mut value = fixture();
    value["outputDsp"][1]["effects"][0]["effect"]["gain"] = json!(f64::MAX);
    let host = create(value);
    let data = [bytes(&[0.5; 4]), bytes(&[8.0; 4]), bytes(&[0.25; 4])];
    assert_eq!(run(&host, 0, &inputs(&data, stamp(0))), MX_OK);
    let mut packet = std::mem::MaybeUninit::uninit();
    assert_eq!(
        unsafe { attestation::mx_host_output_packet_v1(host.0, 0, packet.as_mut_ptr()) },
        MX_OK
    );
    assert_eq!(unsafe { *packet.assume_init().samples }, 0.25);
    assert_eq!(
        unsafe { attestation::mx_host_output_packet_v1(host.0, 1, packet.as_mut_ptr()) },
        MX_INACTIVE
    );
    let packet = unsafe { packet.assume_init() };
    assert!(packet.samples.is_null());
    assert_eq!(packet.sample_count, 0);
    assert_eq!(packet.attestation_sha256, [0; 32]);
}
#[test]
fn source_loss_discards_tail_without_shrinking_immutable_packet_union() {
    let mut value = fixture();
    let delay = json!({"name":"retained","format":{"sampleRateHz":48000,"channels":["FL","FR"]},"effects":[{"name":"delay","enabled":true,"effect":{"kind":"delay","frames":4,"wet":1.0,"dry":0.0,"feedback":0.0}}]});
    value["mixDsp"][1] = delay.clone();
    value["outputDsp"][1] = delay;
    let host = create(value);
    let (before, record) = attested(&host, 1);
    assert_eq!(
        contributor_ids(&record),
        ["owned-source-return", "owned-source-shared"]
    );
    let data = blocks();
    assert_eq!(run(&host, 0, &inputs(&data, stamp(0))), MX_OK);
    let mut input = inputs(&data, stamp(1));
    input[2].bytes = ptr::null();
    input[2].byte_count = 0;
    assert_eq!(run(&host, 1, &input), MX_OK);
    assert_eq!(output(&host, 0), [0.0; 4]);
    assert_eq!(output(&host, 1), [0.0; 4]);
    assert_eq!(attested(&host, 1).0, before);
    let mut packet = std::mem::MaybeUninit::uninit();
    assert_eq!(
        unsafe { attestation::mx_host_output_packet_v1(host.0, 1, packet.as_mut_ptr()) },
        MX_OK
    );
    let packet = unsafe { packet.assume_init() };
    assert_eq!(packet.stamp.sequence, 1);
    assert_eq!(
        packet.attestation_sha256,
        <[u8; 32]>::from(sha2::Sha256::digest(&before))
    );
}
