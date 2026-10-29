mod support;
use mixyard_network::*;
use support::*;

#[test]
fn canonical_raw_descriptor_is_independent_of_destination_packet_size() -> anyhow::Result<()> {
    // Every sample frame alone exceeds a v1 datagram. A separately bounded
    // fragmented destination may still carry this exact descriptor unchanged.
    let channels: Vec<_> = (0..160).map(|n| format!("ordered-{n}")).collect();
    let names: Vec<_> = channels.iter().map(String::as_str).collect();
    let original = spec(1, "a", Codec::RawF64Le, 44_117, &names, 2);
    original.validate_descriptor("a", "b")?;
    assert_eq!(original.payload_bytes()?, 2560);
    assert!(original.validate("a", "b", &limits()).is_err());
    assert_eq!(original.channels, channels);
    let decoded: Spec = serde_json::from_slice(&serde_json::to_vec(&original)?)?;
    assert_eq!(decoded, original);
    Ok(())
}

#[test]
fn framing_independent_validation_still_rejects_unsafe_identity_and_format() {
    let good = spec(1, "a", Codec::RawS24Le, 96_000, &["FR", "FL"], 8);
    let mut bad = good.clone();
    bad.codec = Codec::Opus;
    assert!(bad.validate_descriptor("a", "b").is_err());
    bad = good.clone();
    bad.provenance.visited_peers.push("b".into());
    assert!(bad.validate_descriptor("b", "b").is_err());
    assert!(bad.validate_descriptor("a", "b").is_err());
    bad = good.clone();
    bad.provenance
        .sources
        .push(bad.provenance.sources[0].clone());
    assert!(bad.validate_descriptor("a", "b").is_err());
    bad = good.clone();
    bad.channels.push("FR".into());
    assert!(bad.validate_descriptor("a", "b").is_err());
    bad = good.clone();
    bad.sample_rate = 0;
    assert!(bad.validate_descriptor("a", "b").is_err());
    bad = good.clone();
    bad.frames_per_packet = 0;
    assert!(bad.validate_descriptor("a", "b").is_err());
    bad = good;
    bad.provenance.sources[0].peer = "unvisited-origin".into();
    assert!(bad.validate_descriptor("a", "b").is_err());
}
