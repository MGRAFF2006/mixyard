#[path = "../tests/support/mod.rs"]
mod support;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    support::raw_proof().await?;
    println!(
        "PASS: mutually authenticated loopback QUIC; two independent outbound streams + return stream; raw S24/96k/6ch, F64/192k/4ch, S32/44.1k/stereo bytes exact; generation restart; disconnect silence. No native audio or latency qualification."
    );
    Ok(())
}
