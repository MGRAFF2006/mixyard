mod support;
use anyhow::Result;
use mixyard_network::{fragment_transport::*, fragmentation::Config, *};
use std::{sync::Arc, time::Duration};
use support::*;

fn config() -> Config {
    Config {
        max_block_bytes: 128 * 1024,
        max_datagram: 1000,
        max_streams: 16,
        window: 8,
        max_buffered_bytes: 2 * 1024 * 1024,
        timeout: Duration::from_secs(1),
    }
}
async fn block_pair(
    send: &[Spec],
    receive: &[Spec],
    other_datagram: usize,
) -> Result<(
    Arc<FragmentSession>,
    Arc<FragmentSession>,
    FragmentEndpoint,
    FragmentEndpoint,
)> {
    let a = identity("a")?;
    let b = identity("b")?;
    let pin_a = PairedPeer {
        peer: "a".into(),
        certificate: a.certificate.clone(),
        server_name: "mixyard-peer".into(),
    };
    let pin_b = PairedPeer {
        peer: "b".into(),
        certificate: b.certificate.clone(),
        server_name: "mixyard-peer".into(),
    };
    let mut budget = limits();
    budget.max_bytes_per_second = 200_000_000;
    let ea = fragment_endpoint(
        "127.0.0.1:0".parse()?,
        a,
        pin_b,
        permissions("a", "b", send, receive),
        budget.clone(),
        config(),
    )?;
    let mut second = config();
    second.max_datagram = other_datagram;
    let eb = fragment_endpoint(
        "127.0.0.1:0".parse()?,
        b,
        pin_a,
        permissions("b", "a", receive, send),
        budget,
        second,
    )?;
    let (a, b) = tokio::try_join!(ea.connect(eb.local_addr()?), eb.accept())?;
    Ok((a, b, ea, eb))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_fragmented_bidirectional_blocks_preserve_exact_spec_and_raw_bits()
-> Result<()> {
    let channels: Vec<_> = (0..160).map(|n| format!("C{n}")).collect();
    let names: Vec<_> = channels.iter().map(String::as_str).collect();
    // A single sample frame is 1280 bytes, greater than the path datagram.
    let wide = spec(1, "a", Codec::RawF64Le, 44_117, &names, 2);
    let sibling = spec(2, "a", Codec::RawS24Le, 96_000, &["FR", "FL", "aux"], 512);
    let reverse = spec(3, "b", Codec::RawS32Le, 192_000, &["mono"], 512);
    let (a, b, _ea, _eb) = block_pair(
        &[wide.clone(), sibling.clone()],
        std::slice::from_ref(&reverse),
        1000,
    )
    .await?;
    let arun = tokio::spawn(a.clone().run());
    let brun = tokio::spawn(b.clone().run());
    for s in [&wide, &sibling] {
        a.negotiate(offer(s.clone(), 1)).await?;
    }
    b.negotiate(offer(reverse.clone(), 1)).await?;
    let file_path =
        std::env::temp_dir().join(format!("mixyard-fragment-{}.pcm", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file_path)?;
    let mut file_expected = Vec::new();
    for sequence in 0..4 {
        let first = sample_bytes(&wide, sequence);
        let second = sample_bytes(&sibling, sequence);
        let returned = sample_bytes(&reverse, sequence);
        tokio::try_join!(
            a.send(1, 1, &first),
            a.send(2, 1, &second),
            b.send(3, 1, &returned)
        )?;
        for (id, s, expected) in [(1, &wide, &first), (2, &sibling, &second)] {
            let (actual_spec, block) = b.receive(id, 1, sequence as u64).await?;
            assert_eq!(&actual_spec, s);
            assert_eq!(
                block.sample_timestamp,
                sequence as u64 * u64::from(s.frames_per_packet)
            );
            assert_eq!(&block.payload, expected);
            if id == 1 {
                std::io::Write::write_all(&mut file, &block.payload)?;
                file_expected.extend_from_slice(expected);
            }
            assert!(
                b.take(id, 1, sequence as u64)?.is_none(),
                "completed block cannot replay"
            );
            b.advance(id, 1, sequence as u64 + 1)?;
        }
        let (actual_spec, block) = a.receive(3, 1, sequence as u64).await?;
        assert_eq!(actual_spec, reverse);
        assert_eq!(block.payload, returned);
        a.advance(3, 1, sequence as u64 + 1)?;
    }
    drop(file);
    let file_actual = std::fs::read(&file_path)?;
    std::fs::remove_file(&file_path)?;
    assert_eq!(
        file_actual, file_expected,
        "actual file destination preserves every opaque bit"
    );
    a.negotiate(offer(wide.clone(), 2)).await?;
    assert!(
        b.take(1, 1, 0).is_err(),
        "old generation cannot consume new admission"
    );
    a.send(1, 2, &sample_bytes(&wide, 7)).await?;
    assert_eq!(b.receive(1, 2, 0).await?.1.payload, sample_bytes(&wide, 7));
    a.revoke_send(1)?;
    assert!(a.send(1, 2, &sample_bytes(&wide, 8)).await.is_err());
    a.send(2, 1, &sample_bytes(&sibling, 5)).await?;
    assert_eq!(
        b.receive(2, 1, 4).await?.1.payload,
        sample_bytes(&sibling, 5)
    );
    b.revoke_receive(1)?;
    assert!(b.take(1, 2, 0).is_err());
    a.close();
    assert!(a.send(2, 1, &sample_bytes(&sibling, 6)).await.is_err());
    let _ = arun.await?;
    let _ = brun.await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fragment_size_and_directional_grants_require_exact_reliable_agreement() -> Result<()> {
    let s = spec(1, "a", Codec::RawF64Le, 44_117, &["FR", "FL"], 256);
    let (a, b, _ea, _eb) = block_pair(std::slice::from_ref(&s), &[], 900).await?;
    let arun = tokio::spawn(a.clone().run());
    let brun = tokio::spawn(b.clone().run());
    assert!(
        b.negotiate(offer(s.clone(), 1)).await.is_err(),
        "receive grant is not send authority"
    );
    assert!(
        a.negotiate(offer(s, 1)).await.is_err(),
        "fragment geometry cannot be silently narrowed"
    );
    assert!(a.take(1, 1, 0).is_err());
    let _ = arun.await?;
    let _ = brun.await?;
    Ok(())
}

#[tokio::test]
async fn actual_fragment_header_rate_and_source_exclusions_gate_admission() -> Result<()> {
    let s = spec(1, "a", Codec::RawS16Le, 10, &["mono"], 1);
    let a = identity("a")?;
    let b = identity("b")?;
    let pin_b = PairedPeer {
        peer: "b".into(),
        certificate: b.certificate,
        server_name: "mixyard-peer".into(),
    };
    let mut budget = limits();
    budget.max_bytes_per_second = 499; // 20 payload bytes/s, but 500 wire bytes/s.
    let rejected = fragment_endpoint(
        "127.0.0.1:0".parse()?,
        a,
        pin_b,
        permissions("a", "b", std::slice::from_ref(&s), &[]),
        budget,
        config(),
    )
    .err()
    .expect("actual headers exceed 499-byte rate budget");
    assert!(rejected.to_string().contains("wire rate"));
    let a = identity("a")?;
    let b = identity("b")?;
    let pin_b = PairedPeer {
        peer: "b".into(),
        certificate: b.certificate,
        server_name: "mixyard-peer".into(),
    };
    let mut exact = limits();
    exact.max_bytes_per_second = 500;
    let admitted = fragment_endpoint(
        "127.0.0.1:0".parse()?,
        a,
        pin_b,
        permissions("a", "b", std::slice::from_ref(&s), &[]),
        exact,
        config(),
    )?;
    drop(admitted);
    let a = identity("a")?;
    let b = identity("b")?;
    let pin_a = PairedPeer {
        peer: "a".into(),
        certificate: a.certificate,
        server_name: "mixyard-peer".into(),
    };
    let mut private = s;
    private.provenance.sources[0]
        .groups
        .push("call-return".into());
    let mut grants = permissions("b", "a", &[], &[private]);
    grants.excluded_receive_groups.insert("call-return".into());
    let rejected = fragment_endpoint("127.0.0.1:0".parse()?, b, pin_a, grants, limits(), config())
        .err()
        .expect("excluded premix cannot be admitted by block framing");
    assert!(rejected.to_string().contains("excluded source"));
    Ok(())
}

// Regressions derived from the independent exact-HEAD feature/safety review.

// Current-thread manual polls make the race deterministic without sleeps.
async fn poll_once<F: std::future::Future>(
    mut future: std::pin::Pin<&mut F>,
) -> std::task::Poll<F::Output> {
    std::future::poll_fn(|cx| std::task::Poll::Ready(future.as_mut().poll(cx))).await
}

#[tokio::test]
async fn replacement_negotiation_reserves_stream_before_reliable_exchange() -> Result<()> {
    let s = spec(1, "a", Codec::RawS16Le, 10, &["mono"], 1000);
    let (a, b, _ea, _eb) = block_pair(std::slice::from_ref(&s), &[], 1000).await?;
    let arun = tokio::spawn(a.clone().run());
    let brun = tokio::spawn(b.clone().run());
    a.negotiate(offer(s.clone(), 1)).await?;
    let negotiation = a.negotiate(offer(s.clone(), 2));
    tokio::pin!(negotiation);
    assert!(poll_once(negotiation.as_mut()).await.is_pending());
    let bytes = sample_bytes(&s, 0);
    let old_send = a.send(1, 1, &bytes);
    tokio::pin!(old_send);
    let old_poll = poll_once(old_send.as_mut()).await;
    // No old send should enter once replacement has begun, or replacement
    // must not commit while that send is in flight.
    let entered = old_poll.is_pending();
    assert!(
        matches!(&old_poll, std::task::Poll::Ready(Err(_))),
        "pending replacement must reject a newly starting old send"
    );
    let result = negotiation.await;
    assert!(
        result.is_ok(),
        "reserved replacement should finish normally: {result:?}"
    );
    let old_result = match old_poll {
        std::task::Poll::Ready(r) => r,
        std::task::Poll::Pending => old_send.await,
    };
    a.close();
    let _ = arun.await?;
    let _ = brun.await?;
    assert!(
        !entered || result.is_err(),
        "new generation committed over an active old send: negotiation={result:?}, old_send={old_result:?}"
    );
    Ok(())
}

async fn single_fragment_queue_pair(
    s: &Spec,
) -> Result<(
    Arc<FragmentSession>,
    Arc<FragmentSession>,
    FragmentEndpoint,
    FragmentEndpoint,
)> {
    let a = identity("a")?;
    let b = identity("b")?;
    let pin_a = PairedPeer {
        peer: "a".into(),
        certificate: a.certificate.clone(),
        server_name: "mixyard-peer".into(),
    };
    let pin_b = PairedPeer {
        peer: "b".into(),
        certificate: b.certificate.clone(),
        server_name: "mixyard-peer".into(),
    };
    let mut budget = limits();
    budget.max_streams = 1;
    budget.packets_per_stream = 1;
    budget.max_datagram = 1000;
    let mut c = config();
    c.max_streams = 1;
    c.window = 1;
    let ea = fragment_endpoint(
        "127.0.0.1:0".parse()?,
        a,
        pin_b,
        permissions("a", "b", std::slice::from_ref(s), &[]),
        budget.clone(),
        c.clone(),
    )?;
    let eb = fragment_endpoint(
        "127.0.0.1:0".parse()?,
        b,
        pin_a,
        permissions("b", "a", &[], std::slice::from_ref(s)),
        budget,
        c,
    )?;
    let (a, b) = tokio::try_join!(ea.connect(eb.local_addr()?), eb.accept())?;
    Ok((a, b, ea, eb))
}

#[tokio::test]
async fn revocation_cancels_capacity_blocked_fragment_before_enqueue() -> Result<()> {
    // Exactly two full fragments; the transport queue can only hold one.
    let s = spec(1, "a", Codec::RawS16Le, 10, &["mono"], 952);
    let (a, b, _ea, _eb) = single_fragment_queue_pair(&s).await?;
    let arun = tokio::spawn(a.clone().run());
    let brun = tokio::spawn(b.clone().run());
    a.negotiate(offer(s.clone(), 1)).await?;
    let bytes = sample_bytes(&s, 0);
    let send = a.send(1, 1, &bytes);
    tokio::pin!(send);
    assert!(
        poll_once(send.as_mut()).await.is_pending(),
        "first fragment yields"
    );
    assert!(
        poll_once(send.as_mut()).await.is_pending(),
        "second fragment waits for queue capacity"
    );
    // No driver tick has occurred between these synchronous polls.
    a.revoke_send(1)?;
    let completed = poll_once(send.as_mut()).await;
    assert!(
        matches!(&completed, std::task::Poll::Ready(Err(_))),
        "revoked pending fragment must stop before a driver tick"
    );
    let result = match completed {
        std::task::Poll::Ready(r) => r,
        std::task::Poll::Pending => send.await,
    };
    let received = b.receive(1, 1, 0).await;
    a.close();
    let _ = arun.await?;
    let _ = brun.await?;
    assert!(
        result.is_err(),
        "revoked blocked send reported success; remote completed={}",
        received.is_ok()
    );
    Ok(())
}

#[tokio::test]
async fn cancelled_fragmented_send_preserves_sibling_and_requires_new_generation() -> Result<()> {
    let s = spec(1, "a", Codec::RawS16Le, 10, &["mono"], 1000);
    let sibling = spec(2, "a", Codec::RawS16Le, 10, &["mono"], 1000);
    let (a, b, _ea, _eb) = block_pair(&[s.clone(), sibling.clone()], &[], 1000).await?;
    let arun = tokio::spawn(a.clone().run());
    let brun = tokio::spawn(b.clone().run());
    a.negotiate(offer(s.clone(), 1)).await?;
    a.negotiate(offer(sibling.clone(), 1)).await?;
    let bytes = sample_bytes(&s, 0);
    {
        let send = a.send(1, 1, &bytes);
        tokio::pin!(send);
        assert!(poll_once(send.as_mut()).await.is_pending());
    }
    assert!(a.send(1, 1, &bytes).await.is_err());
    a.send(2, 1, &sample_bytes(&sibling, 0)).await?;
    assert_eq!(
        b.receive(2, 1, 0).await?.1.payload,
        sample_bytes(&sibling, 0)
    );
    a.negotiate(offer(s.clone(), 2)).await?;
    a.send(1, 2, &bytes).await?;
    assert_eq!(b.receive(1, 2, 0).await?.1.payload, bytes);
    a.close();
    let _ = arun.await?;
    let _ = brun.await?;
    Ok(())
}
