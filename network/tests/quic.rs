mod support;
use mixyard_network::*;
use support::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn encrypted_paired_bidirectional_raw_multistream_exercise() -> anyhow::Result<()> {
    raw_proof().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tls_rejects_an_unpaired_peer_and_wrong_expected_server() -> anyhow::Result<()> {
    let a = identity("a")?;
    let b = identity("b")?;
    let stranger = identity("stranger")?;
    // Client trusts the real server, but presents an unpaired certificate for peer a.
    let client = endpoint(
        "127.0.0.1:0".parse()?,
        Identity {
            peer: "a".into(),
            ..stranger
        },
        PairedPeer {
            peer: "b".into(),
            certificate: b.certificate.clone(),
            server_name: "mixyard-peer".into(),
        },
        permissions("a", "b", &[], &[]),
        limits(),
    )?;
    let server = endpoint(
        "127.0.0.1:0".parse()?,
        b,
        PairedPeer {
            peer: "a".into(),
            certificate: a.certificate.clone(),
            server_name: "mixyard-peer".into(),
        },
        permissions("b", "a", &[], &[]),
        limits(),
    )?;
    let (client_result, server_result) =
        tokio::join!(client.connect(server.local_addr()?), server.accept());
    assert!(
        server_result.is_err(),
        "server must reject unknown client before admitting any stream"
    );
    // TLS client may complete locally before receiving the server's rejection alert.
    if let Ok(client_session) = client_result {
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), client_session.run())
                .await?
                .is_err()
        );
    }
    let wrong_pin = identity("wrong")?;
    let impostor = endpoint(
        "127.0.0.1:0".parse()?,
        a,
        PairedPeer {
            peer: "b".into(),
            certificate: wrong_pin.certificate,
            server_name: "mixyard-peer".into(),
        },
        permissions("a", "b", &[], &[]),
        limits(),
    )?;
    let (client_result, _) = tokio::join!(impostor.connect(server.local_addr()?), server.accept());
    assert!(
        client_result.is_err(),
        "client must verify expected server pin"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_and_remote_directional_stream_permissions_are_required() -> anyhow::Result<()> {
    let s = spec(1, "a", Codec::RawS16Le, 48_000, &["FL", "FR"], 48);
    let (a, b, _ea, _eb) = pair(std::slice::from_ref(&s), &[]).await?;
    let run_a = tokio::spawn(a.clone().run());
    let run_b = tokio::spawn(b.clone().run());
    assert!(b.negotiate(offer(s.clone(), 1)).await.is_err()); // receive grant never permits sending
    let mut unpermitted = s.clone();
    unpermitted.stream = 999;
    assert!(a.negotiate(offer(unpermitted, 1)).await.is_err());
    a.negotiate(offer(s.clone(), 1)).await?;
    assert!(a.negotiate(offer(s.clone(), 1)).await.is_err());
    assert!(a.enqueue(999, vec![0; 192]).await.is_err());
    a.enqueue(1, sample_bytes(&s, 0)).await?;
    a.flush_round().await?;
    wait_queued(&b, 1, 1).await?;
    assert_eq!(b.pull(1)?, sample_bytes(&s, 0));
    a.close();
    let _ = run_a.await?;
    let _ = run_b.await?;
    Ok(())
}
