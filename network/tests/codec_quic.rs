#[path = "support/mod.rs"]
mod common;
#[path = "codec/support.rs"]
mod support;
use anyhow::Result;
use mixyard_network::{codec_transport::*, *};
use std::time::{Duration, Instant};
use support::*;

#[test]
fn request_v1_roundtrips_full_u64_and_rejects_authority_or_ambiguous_consent() -> Result<()> {
    let raw = r#"{"version":1,"streamId":"18446744073709551615","generation":"9007199254740993","encoding":{"mode":"raw"},"allowLossy":false}"#;
    let parsed: CodecRequest = serde_json::from_str(raw)?;
    assert_eq!(parsed.stream_id(), u64::MAX);
    assert_eq!(parsed.generation(), 9_007_199_254_740_993);
    assert_eq!(serde_json::to_string(&parsed)?, raw);
    let lossless = r#"{"version":1,"streamId":"4","generation":"2","encoding":{"mode":"zstd","level":3},"allowLossy":false}"#;
    let lossy = r#"{"version":1,"streamId":"4","generation":"2","encoding":{"mode":"opus","bitrate":128000,"complexity":5,"vbr":false,"conversion":"normalized-f32-clamp-v1"},"allowLossy":true}"#;
    for text in [lossless, lossy] {
        assert_eq!(
            serde_json::to_string(&serde_json::from_str::<CodecRequest>(text)?)?,
            text
        );
    }
    let mut invalid = vec![
        raw.replace("\"version\":1", "\"version\":2"),
        raw.replace(
            "\"streamId\":\"18446744073709551615\"",
            "\"streamId\":18446744073709551615",
        ),
        raw.replace("18446744073709551615", "18446744073709551616"),
        raw.replace("18446744073709551615", "01"),
        raw.replace("18446744073709551615", "+1"),
        raw.replace("18446744073709551615", "0"),
        raw.replace("9007199254740993", " 1"),
        raw.replace("\"allowLossy\":false", "\"allowLossy\":true"),
        raw.replace("\"mode\":\"raw\"", "\"mode\":\"raw\",\"level\":1"),
        raw.replace("\"version\":1", "\"version\":1,\"spec\":{}"),
        raw.replace("\"version\":1", "\"version\":1,\"version\":1"),
        lossless.replace("\"level\":3", "\"level\":4"),
        lossy.replace("\"allowLossy\":true", "\"allowLossy\":false"),
        lossy.replace("normalized-f32-clamp-v1", "implicit"),
        lossy.replace("\"complexity\":5", "\"complexity\":11"),
    ];
    invalid.push(raw.replace(
        "\"allowLossy\":false",
        "\"allowLossy\":false,\"authority\":\"admin\"",
    ));
    for text in invalid {
        assert!(
            serde_json::from_str::<CodecRequest>(&text).is_err(),
            "accepted {text}"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raw_default_uses_real_authentication_exact_grants_pacing_and_existing_playout()
-> Result<()> {
    let s = spec(
        u64::MAX,
        "a",
        Codec::RawF64Le,
        44_117,
        &["FR", "FL", "height", "aux"],
        1000,
    );
    let req = request(s.stream, 1, Encoding::Raw);
    let (a, b, _ea, _eb) = pair(
        std::slice::from_ref(&s),
        &[],
        std::slice::from_ref(&req),
        &[],
    )
    .await?;
    let arun = tokio::spawn(a.clone().run());
    let brun = tokio::spawn(b.clone().run());
    let anchor = Instant::now() + Duration::from_millis(100);
    a.negotiate_codec(req.clone(), anchor).await?;
    let local = Instant::now();
    let mut p = b.create_codec_playout(
        s.stream,
        1,
        PlayoutConfig {
            anchor: local,
            target_buffer_frames: 0,
        },
    )?;
    assert!(
        b.create_codec_playout(
            s.stream,
            1,
            PlayoutConfig {
                anchor: local,
                target_buffer_frames: 0
            }
        )
        .is_err()
    );
    let before = a.codec_capability(req.clone(), true);
    assert!(before.available);
    assert!(before.measurements.is_none());
    assert!(before.physical_delay_nanos.is_none() && before.network_delay_nanos.is_none());
    let words = [
        0x8000000000000000u64,
        0x7ff8000000001234,
        1,
        0x3ff0000000000001,
        0x7ff0000000000000,
        0xfff0000000000000,
    ];
    let expected: Vec<u8> = words
        .iter()
        .cycle()
        .take(4000)
        .flat_map(|x| x.to_le_bytes())
        .collect();
    a.send_codec(s.stream, 1, &expected).await?;
    assert!(
        Instant::now() >= anchor,
        "actual enqueue must wait for source frame zero"
    );
    service(&b, &mut p).await?;
    assert_eq!(p.spec(), &s);
    let observation = b.codec_timeline_observation(&p)?;
    assert_eq!(
        serde_json::to_value(&observation)?["sourceSpec"]["stream"],
        u64::MAX.to_string()
    );
    assert_eq!(render(&mut p, local, 0), expected);
    assert!(
        b.take(s.stream, 1, 0).is_err(),
        "decoded bytes must use lifecycle-gated playout"
    );
    let metrics = a.codec_capability(req, true);
    assert_eq!(metrics.measurements.as_ref().unwrap().raw_bytes, 32000);
    assert_eq!(metrics.measurements.as_ref().unwrap().envelope_bytes, 32056);
    assert!(metrics.measurements.as_ref().unwrap().fragment_bytes > 32056);
    println!("raw {}", serde_json::to_string(&metrics)?);
    b.revoke_receive(s.stream)?;
    assert_eq!(p.revocation().status(), RevocationStatus::Complete);
    let mut out = vec![0xff; expected.len()];
    p.render_into(local + Duration::from_secs(1), &mut out)?;
    assert!(out.iter().all(|v| *v == 0));
    a.close();
    let _ = arun.await?;
    let _ = brun.await?;
    Ok(())
}

#[cfg(all(feature = "system-zstd", feature = "system-opus"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn paired_socket_mixed_raw_zstd_and_explicit_opus_preserve_independent_sources() -> Result<()>
{
    let raw = spec(1, "a", Codec::RawS32Le, 96_000, &["FR", "FL", "aux"], 1920);
    let zstd = spec(2, "a", Codec::RawF64Le, 44_117, &["height", "aux"], 1000);
    let opus = spec(3, "a", Codec::RawF32Le, 48_000, &["FL", "FR"], 960);
    let mut lossless_s32 = raw.clone();
    lossless_s32.stream = 5;
    lossless_s32.name = "Independent lossless destination for bus 1".into();
    let reverse = spec(
        4,
        "b",
        Codec::RawS24Le,
        192_000,
        &["selected", "aux", "height"],
        1920,
    );
    let requests = [
        request(1, 1, Encoding::Raw),
        request(2, 1, Encoding::Zstd { level: 2 }),
        request(3, 1, support::opus()),
        request(5, 1, Encoding::Zstd { level: 3 }),
    ];
    let back = request(4, 1, Encoding::Zstd { level: 1 });
    let (a, b, _ea, _eb) = pair(
        &[
            raw.clone(),
            zstd.clone(),
            opus.clone(),
            lossless_s32.clone(),
        ],
        std::slice::from_ref(&reverse),
        &requests,
        std::slice::from_ref(&back),
    )
    .await?;
    let arun = tokio::spawn(a.clone().run());
    let brun = tokio::spawn(b.clone().run());
    let anchor = Instant::now() + Duration::from_millis(150);
    for req in &requests {
        a.negotiate_codec(req.clone(), anchor).await?;
    }
    b.negotiate_codec(back, anchor).await?;
    let local = Instant::now();
    let mut ps = [1, 2, 3, 5].map(|id| {
        b.create_codec_playout(
            id,
            1,
            PlayoutConfig {
                anchor: local,
                target_buffer_frames: 0,
            },
        )
        .unwrap()
    });
    let mut pr = a.create_codec_playout(
        4,
        1,
        PlayoutConfig {
            anchor: local,
            target_buffer_frames: 0,
        },
    )?;
    // Independent oracles: exact integer bit patterns and floating NaN payloads,
    // signed zero, infinities, subnormals and >f32 precision bits.
    let raw_expected: Vec<_> = [i32::MIN, 0x12345679, i32::MAX]
        .into_iter()
        .cycle()
        .take(5760)
        .flat_map(i32::to_le_bytes)
        .collect();
    let zstd_expected: Vec<_> = [
        0x7ff8000000001234u64,
        0xfff0000000000000,
        0x8000000000000000,
        1,
        0x3ff0000000000001,
    ]
    .into_iter()
    .cycle()
    .take(2000)
    .flat_map(u64::to_le_bytes)
    .collect();
    let reverse_expected: Vec<_> = [0, 0, 0x80, 0xff, 0xff, 0x7f, 0x35, 0x12, 1]
        .into_iter()
        .cycle()
        .take(reverse.payload_bytes()?)
        .collect();
    let lossy_input: Vec<_> = (0..960)
        .flat_map(|i| {
            let l = (i as f32 * 0.02).sin() * 0.5;
            [l, -l]
        })
        .flat_map(f32::to_le_bytes)
        .collect();
    tokio::try_join!(
        a.send_codec(1, 1, &raw_expected),
        a.send_codec(2, 1, &zstd_expected),
        a.send_codec(3, 1, &lossy_input),
        b.send_codec(4, 1, &reverse_expected),
        a.send_codec(5, 1, &raw_expected)
    )?;
    for p in &mut ps {
        service(&b, p).await?;
    }
    service(&a, &mut pr).await?;
    assert_eq!(render(&mut ps[0], local, 0), raw_expected);
    assert_eq!(render(&mut ps[1], local, 0), zstd_expected);
    assert_eq!(
        render(&mut ps[3], local, 0),
        raw_expected,
        "independent S32 Zstd destination retains every source bit"
    );
    assert_eq!(render(&mut pr, local, 0), reverse_expected);
    let decoded = render(&mut ps[2], local, 0);
    assert_eq!(decoded.len(), lossy_input.len());
    assert!(
        decoded
            .as_chunks::<4>()
            .0
            .iter()
            .all(|s| f32::from_le_bytes(*s).is_finite())
    );
    for (index, req) in requests.iter().enumerate() {
        assert_eq!(ps[index].spec(), [&raw, &zstd, &opus, &lossless_s32][index]);
        let observation = b.codec_timeline_observation(&ps[index])?;
        assert_eq!(&observation.request, req);
        assert_eq!(observation.source_spec, *ps[index].spec());
        assert_eq!(
            observation.next_source_sample_frame,
            ps[index].spec().frames_per_packet.to_string()
        );
        assert_eq!(observation.bit_preserving, index != 2);
        assert_eq!(observation.destination_conversion.is_some(), index == 2);
        if index == 2 {
            assert!(observation.codec_lookahead_frames > 0);
        }
        let tx = a.codec_capability(req.clone(), true);
        let rx = b.codec_capability(req.clone(), false);
        assert!(tx.available && rx.available);
        let m = tx.measurements.as_ref().unwrap();
        if index > 0 {
            assert!(
                m.envelope_bytes < m.raw_bytes,
                "this fixture must measure actual payload reduction"
            );
        }
        assert!(m.wall_nanos > 0);
        #[cfg(all(target_os = "linux", target_pointer_width = "64"))]
        assert!(m.cpu_nanos.is_some_and(|n| n > 0));
        println!("mixed tx {}", serde_json::to_string(&tx)?);
        println!("mixed rx {}", serde_json::to_string(&rx)?);
    }
    a.close();
    let _ = arun.await?;
    let _ = brun.await?;
    Ok(())
}

#[cfg(feature = "system-opus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opus_quality_accounts_for_codec_lookahead_and_explicit_s32_conversion() -> Result<()> {
    let s = spec(10, "a", Codec::RawS32Le, 48_000, &["mono"], 960);
    let req = request(10, 1, support::opus());
    let (a, b, _ea, _eb) = pair(
        std::slice::from_ref(&s),
        &[],
        std::slice::from_ref(&req),
        &[],
    )
    .await?;
    let arun = tokio::spawn(a.clone().run());
    let brun = tokio::spawn(b.clone().run());
    let anchor = Instant::now() + Duration::from_millis(80);
    a.negotiate_codec(req.clone(), anchor).await?;
    let local = Instant::now();
    let mut p = b.create_codec_playout(
        10,
        1,
        PlayoutConfig {
            anchor: local,
            target_buffer_frames: 0,
        },
    )?;
    let mut original = Vec::new();
    let mut decoded = Vec::new();
    for sequence in 0..12u64 {
        let samples: Vec<i32> = (0..960)
            .map(|i| {
                let t = (sequence * 960 + i) as f64 / 48_000.0;
                ((t * 440.0 * std::f64::consts::TAU).sin() * 0.5 * 2147483648.0).round() as i32
            })
            .collect();
        let input: Vec<_> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
        original.extend(samples.into_iter().map(|v| v as f64 / 2147483648.0));
        a.send_codec(10, 1, &input).await?;
        service(&b, &mut p).await?;
        decoded.extend(
            render(&mut p, local, sequence)
                .as_chunks::<4>()
                .0
                .iter()
                .map(|v| i32::from_le_bytes(*v) as f64 / 2147483648.0),
        );
    }
    let cap = a.codec_capability(req, true);
    let lookahead = cap.lookahead_frames.unwrap() as usize;
    assert!(lookahead > 0 && lookahead < 960);
    let mut signal = 0.0;
    let mut error = 0.0;
    for i in 1920..decoded.len() - lookahead {
        signal += original[i].powi(2);
        error += (decoded[i + lookahead] - original[i]).powi(2);
    }
    let snr_db = 10.0 * (signal / error).log10();
    assert!(
        snr_db > 15.0,
        "explicit lossy sine fixture quality: {snr_db}dB"
    );
    assert!(
        cap.measurements.as_ref().unwrap().envelope_bytes
            < cap.measurements.as_ref().unwrap().raw_bytes / 4
    );
    println!(
        "Opus sine SNR {snr_db:.2}dB after codec lookahead {lookahead} frames; conversion normalized S32->f32->S32, not bit preservation, not physical/network latency"
    );
    a.close();
    let _ = arun.await?;
    let _ = brun.await?;
    Ok(())
}
