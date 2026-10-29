#[path = "../tests/support/mod.rs"]
mod support;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    support::playout_proof().await?;
    println!(
        "PASS: actual paired encrypted QUIC -> preallocated caller blocks; three independent streams, S24/96k/6ch + F64/44117Hz/4ch + F32/192k/mono, byte exact across packet/block splits; explicit prebuffer, generation reset, revocation, disconnect. Injected LOCAL monotonic block times; no native audio, synchronized clocks or physical latency qualification."
    );
    Ok(())
}
