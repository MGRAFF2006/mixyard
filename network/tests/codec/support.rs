#![allow(dead_code)]
use super::common;
use anyhow::Result;
pub use common::{identity, limits, permissions, spec};
use mixyard_network::{codec_transport::*, fragment_transport::*, fragmentation::Config, *};
use mixyard_network_codecs::Budget;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub fn config() -> Config {
    Config {
        max_block_bytes: 65_592,
        max_datagram: 1000,
        max_streams: 16,
        window: 4,
        max_buffered_bytes: 4 * 1024 * 1024,
        timeout: Duration::from_secs(1),
    }
}
pub fn codec_config(send: Vec<CodecRequest>, receive: Vec<CodecRequest>) -> CodecConfig {
    CodecConfig {
        grants: CodecGrants { send, receive },
        budget: Budget {
            max_block_bytes: 65_536,
            max_encoded_bytes: 65_536,
        },
        max_work_wall: Duration::from_millis(100),
    }
}
pub fn request(stream: u64, generation: u64, encoding: Encoding) -> CodecRequest {
    CodecRequest::new(
        stream,
        generation,
        encoding,
        matches!(encoding, Encoding::Opus { .. }),
    )
    .unwrap()
}
pub fn opus() -> Encoding {
    Encoding::Opus {
        bitrate: 128_000,
        complexity: 5,
        vbr: false,
        conversion: Conversion::NormalizedF32ClampV1,
    }
}
pub async fn pair(
    send: &[Spec],
    receive: &[Spec],
    a_requests: &[CodecRequest],
    b_requests: &[CodecRequest],
) -> Result<(
    Arc<FragmentSession>,
    Arc<FragmentSession>,
    FragmentEndpoint,
    FragmentEndpoint,
)> {
    pair_configured(
        send,
        receive,
        a_requests,
        b_requests,
        a_requests,
        b_requests,
        config(),
        limits(),
    )
    .await
}
#[allow(clippy::too_many_arguments)]
pub async fn pair_configured(
    send: &[Spec],
    receive: &[Spec],
    a_send: &[CodecRequest],
    b_send: &[CodecRequest],
    b_receive: &[CodecRequest],
    a_receive: &[CodecRequest],
    config: Config,
    mut limits: Limits,
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
    limits.max_bytes_per_second = 200_000_000;
    let ea = codec_endpoint(
        "127.0.0.1:0".parse()?,
        a,
        pin_b,
        permissions("a", "b", send, receive),
        limits.clone(),
        config.clone(),
        codec_config(a_send.to_vec(), a_receive.to_vec()),
    )?;
    let eb = codec_endpoint(
        "127.0.0.1:0".parse()?,
        b,
        pin_a,
        permissions("b", "a", receive, send),
        limits,
        config,
        codec_config(b_send.to_vec(), b_receive.to_vec()),
    )?;
    let (a, b) = tokio::try_join!(ea.connect(eb.local_addr()?), eb.accept())?;
    Ok((a, b, ea, eb))
}
pub async fn service(session: &FragmentSession, playout: &mut Playout) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if session.service_codec_playout(playout)? > 0 {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await?
}
pub fn render(playout: &mut Playout, anchor: Instant, sequence: u64) -> Vec<u8> {
    let spec = playout.spec();
    let ns = (u128::from(sequence) * u128::from(spec.frames_per_packet) * 1_000_000_000)
        .div_ceil(u128::from(spec.sample_rate));
    let mut out = vec![0xff; spec.payload_bytes().unwrap()];
    let report = playout
        .render_into(anchor + Duration::from_nanos(ns as u64), &mut out)
        .unwrap();
    assert_eq!(
        report.audio_frames,
        u64::from(playout.spec().frames_per_packet)
    );
    out
}
