//! Actual paired encrypted loopback observations. No devices or physical audio delivery.
#[path = "../tests/support/mod.rs"]
mod support;
use mixyard_network::{
    Codec,
    timing::{RTT_FRESH_FOR, RttValidity},
};
use std::time::Duration;
use support::*;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    let streams = [
        spec(1, "a", Codec::RawS24Le, 96_000, &["FR", "FL"], 48),
        spec(2, "a", Codec::RawF64Le, 192_000, &["FC"], 24),
    ];
    let (a, b, _ea, _eb) = pair(&streams, &[]).await?;
    let run_a = tokio::spawn(a.clone().run());
    let run_b = tokio::spawn(b.clone().run());
    println!(
        "{}",
        serde_json::json!({"stage": "initial", "session": a.timing_snapshot().await})
    );
    for s in &streams {
        a.negotiate(offer(s.clone(), 1)).await?;
        a.enqueue(s.stream, sample_bytes(s, 0)).await?;
    }
    let queued = a.timing_snapshot().await;
    assert_eq!(queued.send_streams[0].queued_sample_ms, 0.5);
    assert_eq!(queued.send_streams[1].queued_sample_ms, 0.125);
    println!(
        "{}",
        serde_json::json!({"stage": "queued_sender", "session": queued})
    );
    a.flush_round().await?;
    for s in &streams {
        wait_queued(&b, s.stream, 1).await?;
    }
    a.probe_rtt().await?;
    assert_eq!(
        a.timing_snapshot().await.application_rtt.validity,
        RttValidity::Fresh
    );
    println!(
        "{}",
        serde_json::json!({"stage": "fresh_application_rtt", "session": a.timing_snapshot().await})
    );
    println!(
        "{}",
        serde_json::json!({"stage": "queued_receiver", "session": b.timing_snapshot().await})
    );
    for s in &streams {
        assert_eq!(b.pull(s.stream)?, sample_bytes(s, 0));
    }
    tokio::time::sleep(RTT_FRESH_FOR + Duration::from_millis(20)).await;
    assert_eq!(
        a.timing_snapshot().await.application_rtt.validity,
        RttValidity::Stale
    );
    println!(
        "{}",
        serde_json::json!({"stage": "stale", "session": a.timing_snapshot().await})
    );
    a.close();
    let _ = run_a.await?;
    let _ = run_b.await?;
    println!(
        "{}",
        serde_json::json!({"stage": "disconnected", "session": a.timing_snapshot().await})
    );
    Ok(())
}
