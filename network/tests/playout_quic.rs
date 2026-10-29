mod support;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn encrypted_network_into_sample_aligned_blocks() -> anyhow::Result<()> {
    support::playout_proof().await
}
