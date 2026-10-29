use crate::{Receiver, pcm::Outbox, protocol::*, timing::*};
use anyhow::{Context, Result, ensure};
use quinn::{
    Connection, Endpoint, RecvStream, SendStream,
    crypto::rustls::{QuicClientConfig, QuicServerConfig},
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore},
    time::timeout,
};

/// Provisioned externally. Production pairing/key storage are intentionally not implemented.
pub struct Identity {
    pub peer: String,
    pub certificate: CertificateDer<'static>,
    pub private_key: PrivateKeyDer<'static>,
}

#[derive(Clone)]
pub struct PairedPeer {
    pub peer: String,
    pub certificate: CertificateDer<'static>,
    pub server_name: String,
}

pub struct SecureEndpoint {
    endpoint: Endpoint,
    peer: PairedPeer,
    permissions: Permissions,
    limits: Limits,
    slots: Arc<Semaphore>,
}

/// Mutual TLS 1.3, normal WebPKI signature/name/time validation AND exact leaf pin.
/// No accept-any verifier, 0-RTT, discovery, or public rendezvous.
pub fn endpoint(
    bind: SocketAddr,
    identity: Identity,
    peer: PairedPeer,
    permissions: Permissions,
    limits: Limits,
) -> Result<SecureEndpoint> {
    permissions.validate(&limits)?;
    ensure!(
        identity.peer == permissions.local_peer && peer.peer == permissions.remote_peer,
        "certificate identity and permission peer mismatch"
    );
    ensure!(
        limits.max_connections <= Semaphore::MAX_PERMITS,
        "connection budget exceeds semaphore capacity"
    );
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots.add(peer.certificate.clone())?;
    let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
        Arc::new(roots.clone()),
        provider.clone(),
    )
    .build()?;
    let mut server_tls = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![identity.certificate.clone()],
            identity.private_key.clone_key(),
        )?;
    server_tls.alpn_protocols = vec![ALPN.to_vec()];
    server_tls.max_early_data_size = 0;
    let mut client_tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_root_certificates(roots)
        .with_client_auth_cert(vec![identity.certificate], identity.private_key)?;
    client_tls.alpn_protocols = vec![ALPN.to_vec()];
    client_tls.enable_early_data = false;
    let mut transport = quinn::TransportConfig::default();
    transport.datagram_receive_buffer_size(Some(limits.transport_queue_bytes()?));
    transport.datagram_send_buffer_size(limits.transport_queue_bytes()?);
    // One active reliable admission exchange. Extra controls are reset; PCM is independent.
    transport.max_concurrent_bidi_streams(2u32.into());
    transport.max_concurrent_uni_streams(0u32.into());
    transport.stream_receive_window((CONTROL_LIMIT as u32 + 4).into());
    transport.receive_window((2 * CONTROL_LIMIT as u32 + 8).into());
    transport.send_window((2 * CONTROL_LIMIT + 8) as u64);
    transport.max_idle_timeout(Some(Duration::from_secs(5).try_into()?));
    let transport = Arc::new(transport);
    let mut server =
        quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(server_tls)?));
    server.transport_config(transport.clone());
    server.max_incoming(limits.max_connections);
    server.incoming_buffer_size(16 * 1024);
    server.incoming_buffer_size_total(
        (limits.max_connections as u64)
            .checked_mul(16 * 1024)
            .context("handshake budget overflow")?,
    );
    let mut client = quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(client_tls)?));
    client.transport_config(transport);
    let mut endpoint = Endpoint::server(server, bind)?;
    endpoint.set_default_client_config(client);
    let slots = Arc::new(Semaphore::new(limits.max_connections));
    Ok(SecureEndpoint {
        endpoint,
        peer,
        permissions,
        limits,
        slots,
    })
}

impl SecureEndpoint {
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.endpoint.local_addr()?)
    }

    pub async fn connect(&self, address: SocketAddr) -> Result<Arc<Session>> {
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .context("connection admission limit")?;
        let connection = timeout(
            self.limits.control_timeout,
            self.endpoint.connect(address, &self.peer.server_name)?,
        )
        .await
        .context("TLS handshake timeout")??;
        self.session(connection, permit)
    }

    pub async fn accept(&self) -> Result<Arc<Session>> {
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .context("connection admission limit")?;
        let incoming = self.endpoint.accept().await.context("endpoint closed")?;
        let connection = timeout(self.limits.control_timeout, incoming)
            .await
            .context("TLS handshake timeout")??;
        self.session(connection, permit)
    }

    fn session(
        &self,
        connection: Connection,
        permit: OwnedSemaphorePermit,
    ) -> Result<Arc<Session>> {
        let identity = connection
            .peer_identity()
            .context("missing TLS peer certificate")?
            .downcast::<Vec<CertificateDer<'static>>>()
            .map_err(|_| anyhow::anyhow!("unexpected TLS identity type"))?;
        if identity.first() != Some(&self.peer.certificate) {
            connection.close(1u32.into(), b"unpaired peer");
            anyhow::bail!("certificate pin mismatch");
        }
        ensure!(
            connection.max_datagram_size().is_some(),
            "peer does not support QUIC DATAGRAM"
        );
        if connection.datagram_send_buffer_space() < self.limits.max_datagram {
            connection.close(1u32.into(), b"QUIC queue cannot fit a configured packet");
            anyhow::bail!(
                "QUIC queue cannot hold the configured datagram budget including transport metadata"
            );
        }
        Ok(Arc::new(Session {
            receiver: Mutex::new(Receiver::new(
                self.permissions.clone(),
                self.limits.clone(),
            )?),
            outgoing: Mutex::new(Outbox::new(self.limits.packets_per_stream)),
            admission: tokio::sync::Mutex::new(()),
            running: AtomicBool::new(false),
            probe_slot: Semaphore::new(1),
            probe: Mutex::new(ProbeState::new()),
            connection,
            permissions: self.permissions.clone(),
            limits: self.limits.clone(),
            _permit: permit,
        }))
    }
}

/// Network/codec worker boundary. No devices, mixing, resampling or realtime callbacks.
pub struct Session {
    pub(crate) connection: Connection,
    permissions: Permissions,
    limits: Limits,
    receiver: Mutex<Receiver>,
    outgoing: Mutex<Outbox>,
    admission: tokio::sync::Mutex<()>,
    running: AtomicBool,
    probe_slot: Semaphore,
    probe: Mutex<ProbeState>,
    _permit: OwnedSemaphorePermit,
}

struct PendingAdmission<'a> {
    session: &'a Session,
    confirmed: bool,
}

impl Drop for PendingAdmission<'_> {
    fn drop(&mut self) {
        if !self.confirmed {
            self.session.close();
        }
    }
}

impl Session {
    /// Keep running while the session is usable. Cancellation closes the session and flushes PCM.
    /// At most one bounded control future runs; slow control does not stall datagrams.
    /// Poll it here so publishing reply FIN and releasing the control slot occur in one turn.
    pub async fn run(self: Arc<Self>) -> Result<()> {
        ensure!(
            !self.running.swap(true, Ordering::AcqRel),
            "session receive worker already started"
        );
        struct CloseOnDrop(Arc<Session>);
        impl Drop for CloseOnDrop {
            fn drop(&mut self) {
                self.0.close();
            }
        }
        let _close_on_drop = CloseOnDrop(self.clone());
        let mut control: Option<Pin<Box<dyn Future<Output = Result<()>> + Send>>> = None;
        let mut control_window = Instant::now();
        let mut control_count = 0;
        // Allow a full configured stream-admission burst plus bounded telemetry headroom.
        let control_budget = self.limits.max_streams.saturating_add(16);
        let result = loop {
            tokio::select! {
                incoming = self.connection.accept_bi(), if control.is_none() => match incoming {
                    Ok((mut send, mut receive)) => {
                        // One active exchange; Quinn permits at most two concurrent
                        // inbound streams, so a successor waits in bounded transport
                        // storage instead of resetting valid admission after a probe.
                        let now = Instant::now();
                        if now.duration_since(control_window) >= Duration::from_secs(1) {
                            control_window = now;
                            control_count = 0;
                        }
                        control_count += 1;
                        if control_count > control_budget {
                            self.close();
                            break Err(anyhow::anyhow!("control stream rate exceeded"));
                        }
                        let session = self.clone();
                        control = Some(Box::pin(async move {
                            timeout(session.limits.control_timeout, async {
                                let mut prefix = [0; 4];
                                receive.read_exact(&mut prefix).await?;
                                if prefix == PROBE_TAG {
                                    let nonce = read_probe_body(&mut receive, PROBE_PING).await?;
                                    // A valid optional probe changes no admission state.
                                    // Its caller may stop receiving after timeout/cancel;
                                    // failure to deliver the echo must not stop PCM.
                                    let _ = write_probe(&mut send, PROBE_ECHO, nonce).await;
                                    let _ = send.finish();
                                    return Ok(());
                                } else {
                                    let offer: Offer = read_control_body(&mut receive, prefix).await?;
                                    let admission = session.receiver.lock().expect("receiver poisoned").admit(offer);
                                    let reply = Reply { version: VERSION, accepted: admission.is_ok(), reason: admission.err().map_or_else(String::new, |e| e.to_string()) };
                                    write_control(&mut send, &reply).await?;
                                }
                                send.finish()?;
                                Ok::<_, anyhow::Error>(())
                            }).await.context("control deadline expired")?
                        }));
                    },
                    Err(e) => break Err(e.into()),
                },
                datagram = self.connection.read_datagram() => match datagram {
                    Ok(data) => self.receiver.lock().expect("receiver poisoned").ingest(&data),
                    Err(e) => break Err(e.into()),
                },
                result = async { control.as_mut().expect("active control").await }, if control.is_some() => {
                    control = None;
                    if let Err(error) = result {
                        // Uncertain admission or malformed probes fail closed.
                        break Err(error);
                    }
                }
            }
        };
        drop(control);
        self.receiver
            .lock()
            .expect("receiver poisoned")
            .disconnect();
        self.outgoing.lock().expect("outgoing poisoned").clear();
        result
    }

    /// A bounded authenticated application RTT, including reliable-control and peer scheduling.
    /// Explicitly caller-triggered; no background traffic. At most one probe and one start/second.
    /// Failure/cancellation changes no PCM admission and never supplies a fabricated RTT.
    pub async fn probe_rtt(&self) -> Result<Duration> {
        ensure!(self.connection.close_reason().is_none(), "disconnected");
        let _slot = self
            .probe_slot
            .try_acquire()
            .context("probe already in flight")?;
        let started = Instant::now();
        let nonce = {
            let mut state = self.probe.lock().expect("probe poisoned");
            ensure!(
                state
                    .last_attempt
                    .is_none_or(|at| started.duration_since(at) >= PROBE_INTERVAL),
                "probe rate exceeded"
            );
            state.nonce = state
                .nonce
                .checked_add(1)
                .context("probe nonce exhausted")?;
            state.last_attempt = Some(started);
            state.status = ProbeStatus::InFlight;
            state.nonce
        };
        struct CancelProbe<'a>(&'a Mutex<ProbeState>, bool);
        impl Drop for CancelProbe<'_> {
            fn drop(&mut self) {
                if !self.1 {
                    self.0.lock().expect("probe poisoned").status = ProbeStatus::Cancelled;
                }
            }
        }
        let mut guard = CancelProbe(&self.probe, false);
        let result = timeout(self.limits.control_timeout.min(PROBE_TIMEOUT), async {
            // The peer admits one reliable control exchange at a time. Share
            // this gate with negotiation, without making cancelled probes an
            // uncertain PCM admission. Waiting is bounded but excluded from RTT.
            let _control = self.admission.lock().await;
            let exchange_started = Instant::now();
            let (mut send, mut receive) = self.connection.open_bi().await?;
            write_probe(&mut send, PROBE_PING, nonce).await?;
            send.finish()?;
            let mut prefix = [0; 4];
            receive.read_exact(&mut prefix).await?;
            ensure!(prefix == PROBE_TAG, "invalid probe reply tag");
            ensure!(
                read_probe_body(&mut receive, PROBE_ECHO).await? == nonce,
                "probe nonce mismatch"
            );
            ensure!(self.connection.close_reason().is_none(), "disconnected");
            Ok::<_, anyhow::Error>(exchange_started.elapsed())
        })
        .await;
        let completed = Instant::now();
        let mut state = self.probe.lock().expect("probe poisoned");
        guard.1 = true;
        match result {
            Ok(Ok(elapsed)) => {
                state.sample = Some((completed, elapsed));
                state.status = ProbeStatus::Succeeded;
                Ok(elapsed)
            }
            Ok(Err(error)) => {
                state.status = ProbeStatus::Failed;
                Err(error)
            }
            Err(error) => {
                state.status = ProbeStatus::TimedOut;
                Err(error.into())
            }
        }
    }

    /// Allocating, locking network-worker snapshot. Never call from a realtime callback.
    /// The two queues are observed separately; this is not an atomic audio-path measurement.
    pub async fn timing_snapshot(&self) -> SessionTiming {
        let mut outgoing = self.outgoing.lock().expect("outgoing poisoned");
        let mut receiver = self.receiver.lock().expect("receiver poisoned");
        let connected = self.connection.close_reason().is_none();
        if !connected {
            outgoing.clear();
            receiver.disconnect();
        }
        let probe = self.probe.lock().expect("probe poisoned");
        let now = Instant::now();
        SessionTiming {
            connected,
            observed_at_session_ms: ms(now.duration_since(probe.origin)),
            application_rtt: probe.snapshot(now, connected),
            clock_sync: ClockTiming {
                state: ClockSyncState::Unavailable,
                offset_ms: None,
                uncertainty_ms: None,
            },
            one_way_network_ms: None,
            end_to_end_audio_ms: None,
            send_streams: outgoing.timing_snapshot(),
            receive_streams: receiver.timing_snapshot(),
            rejected: receiver.rejected.clone(),
        }
    }

    pub async fn negotiate(&self, offer: Offer) -> Result<()> {
        ensure!(
            offer.version == VERSION && offer.generation > 0,
            "unsupported version/generation"
        );
        ensure!(
            self.permissions.send.get(&offer.spec.stream) == Some(&offer.spec),
            "sender stream/format/provenance not permitted"
        );
        ensure!(
            offer.spec.payload_bytes()? + HEADER
                <= self
                    .connection
                    .max_datagram_size()
                    .context("datagrams unavailable")?,
            "path MTU too small: explicitly renegotiate smaller packet"
        );
        // Serialize outbound admission, so replies cannot commit generations out of order.
        let _admission = self.admission.lock().await;
        ensure!(
            self.outgoing
                .lock()
                .expect("outgoing poisoned")
                .generation(offer.spec.stream)
                .is_none_or(|g| offer.generation > g),
            "generation must increase"
        );
        // Dropping a negotiation future is also an uncertain control outcome.
        let mut admission_guard = PendingAdmission {
            session: self,
            confirmed: false,
        };
        let result = timeout(self.limits.control_timeout, async {
            let (mut send, mut receive) = self.connection.open_bi().await?;
            write_control(&mut send, &offer).await?;
            send.finish()?;
            let reply: Reply = read_control(&mut receive).await?;
            ensure!(
                reply.version == VERSION,
                "unsupported control reply version"
            );
            ensure!(reply.accepted, "peer rejected stream: {}", reply.reason);
            Ok::<_, anyhow::Error>(())
        })
        .await;
        match result {
            Ok(Ok(())) => {
                self.outgoing
                    .lock()
                    .expect("outgoing poisoned")
                    .admit(offer);
                admission_guard.confirmed = true;
                Ok(())
            }
            failure => {
                self.close();
                match failure {
                    Ok(Err(e)) => Err(e),
                    Err(e) => Err(e.into()),
                    _ => unreachable!(),
                }
            }
        }
    }

    pub async fn enqueue(&self, id: u64, payload: Vec<u8>) -> Result<()> {
        ensure!(
            self.connection.close_reason().is_none(),
            "disconnected: no replay/fallback"
        );
        self.outgoing
            .lock()
            .expect("outgoing poisoned")
            .enqueue(id, payload)
    }

    pub async fn enqueue_pcm(&self, id: u64, pcm: &[u8]) -> Result<usize> {
        ensure!(
            self.connection.close_reason().is_none(),
            "disconnected: no replay/fallback"
        );
        self.outgoing
            .lock()
            .expect("outgoing poisoned")
            .enqueue_pcm(id, pcm)
    }

    /// A fair, nonblocking round. Caller paces by negotiated sample rate; this is not an audio clock.
    pub async fn flush_round(&self) -> Result<usize> {
        ensure!(self.connection.close_reason().is_none(), "disconnected");
        let mut outgoing = self.outgoing.lock().expect("outgoing poisoned");
        let frames = outgoing.round();
        let mut count = 0;
        let mut errors = Vec::new();
        for frame in frames {
            let bytes = frame.encode(self.limits.max_datagram)?;
            // Quinn's send_datagram can evict an older sibling packet when full.
            // Serialize rounds and drop the new packet explicitly instead of causing that eviction.
            if bytes.len() > self.connection.datagram_send_buffer_space() {
                outgoing.congestion_drop(frame.stream);
                continue;
            }
            match self.connection.send_datagram(bytes.into()) {
                Ok(()) => count += 1,
                Err(error) => {
                    outgoing.send_error(frame.stream);
                    errors.push(format!("stream {}: {error}", frame.stream));
                }
            }
        }
        ensure!(
            errors.is_empty(),
            "datagram rejected; no codec fallback; {}",
            errors.join("; ")
        );
        Ok(count)
    }

    pub(crate) fn claim_pacer(&self, offer: &Offer) -> Result<()> {
        self.outgoing
            .lock()
            .expect("outgoing poisoned")
            .claim_pacer(offer)
    }
    pub(crate) fn cancel_sender(&self, offer: &Offer) {
        self.outgoing
            .lock()
            .expect("outgoing poisoned")
            .cancel_sender(offer);
    }

    /// Absolute source slots; never drains or changes an undued sibling stream.
    pub async fn flush_due(&self, due: &[(Offer, u64)]) -> Result<usize> {
        ensure!(self.connection.close_reason().is_none(), "disconnected");
        ensure!(
            due.len() <= self.limits.max_streams,
            "paced stream budget exceeded"
        );
        let mut streams = std::collections::BTreeSet::new();
        ensure!(
            due.iter()
                .all(|(offer, _)| streams.insert(offer.spec.stream)),
            "duplicate stream in paced round"
        );
        let mut outgoing = self.outgoing.lock().expect("outgoing poisoned");
        let mut sent = 0;
        let mut error = None;
        for (offer, sequence) in due {
            // Keep attempting valid siblings after one stream's restart or send error.
            let frame = match outgoing.due(offer, *sequence) {
                Ok(Some(frame)) => frame,
                Ok(None) => continue,
                Err(e) => {
                    error = Some(e);
                    continue;
                }
            };
            let bytes = frame.encode(self.limits.max_datagram)?;
            if bytes.len() > self.connection.datagram_send_buffer_space() {
                outgoing.congestion_drop(frame.stream);
                continue;
            }
            match self.connection.send_datagram(bytes.into()) {
                Ok(()) => sent += 1,
                Err(e) => {
                    outgoing.send_error(frame.stream);
                    error = Some(e.into());
                }
            }
        }
        if let Some(error) = error {
            return Err(error);
        }
        Ok(sent)
    }

    pub async fn sender_offer(&self, id: u64) -> Option<Offer> {
        self.outgoing
            .lock()
            .expect("outgoing poisoned")
            .sender_offer(id)
    }

    pub async fn paced_expired(&self, id: u64) -> u64 {
        self.outgoing
            .lock()
            .expect("outgoing poisoned")
            .paced_expired(id)
    }

    pub(crate) fn worker_stream_limit(&self) -> usize {
        self.limits.max_streams
    }

    pub(crate) fn worker_packet_limit(&self) -> usize {
        self.limits.packets_per_stream
    }

    pub(crate) fn create_worker_playout(
        &self,
        offer: &Offer,
        config: crate::PlayoutConfig,
    ) -> Result<crate::Playout> {
        let mut receiver = self.receiver.lock().expect("receiver poisoned");
        if self.connection.close_reason().is_some() {
            receiver.disconnect();
        }
        receiver.create_playout_for(offer, config)
    }

    pub(crate) fn cancel_receive_generation(&self, offer: &Offer) {
        self.receiver
            .lock()
            .expect("receiver poisoned")
            .cancel_generation(offer);
    }

    pub fn arrival_observation(
        &self,
        id: u64,
        generation: u64,
    ) -> Option<crate::worker::ArrivalObservation> {
        if self.connection.close_reason().is_some() {
            return None;
        }
        self.receiver
            .lock()
            .expect("receiver poisoned")
            .arrival_observation(id, generation)
    }

    pub async fn sender_counters(&self, id: u64) -> Result<crate::Counters> {
        self.outgoing
            .lock()
            .expect("outgoing poisoned")
            .counters(id)
    }

    pub fn pull(&self, id: u64) -> Result<Vec<u8>> {
        let mut receiver = self.receiver.lock().expect("receiver poisoned");
        if self.connection.close_reason().is_some() {
            receiver.disconnect();
        }
        receiver.pull(id)
    }

    /// Allocating worker API. The handle renders without this Session's mutex.
    pub fn create_playout(&self, id: u64, config: crate::PlayoutConfig) -> Result<crate::Playout> {
        let mut receiver = self.receiver.lock().expect("receiver poisoned");
        if self.connection.close_reason().is_some() {
            receiver.disconnect();
        }
        receiver.create_playout(id, config)
    }

    /// Worker-only handoff; do not call from a device callback.
    pub fn service_playout(&self, playout: &mut crate::Playout) -> Result<()> {
        let mut receiver = self.receiver.lock().expect("receiver poisoned");
        if self.connection.close_reason().is_some() {
            receiver.disconnect();
        }
        receiver.service_playout(playout)
    }

    pub fn revoke_receive(&self, id: u64) -> Result<crate::playout::Revocation> {
        self.receiver.lock().expect("receiver poisoned").revoke(id)
    }

    pub fn counters(&self, id: u64) -> Result<crate::Counters> {
        self.receiver
            .lock()
            .expect("receiver poisoned")
            .counters(id)
    }

    pub fn queued(&self, id: u64) -> usize {
        self.receiver.lock().expect("receiver poisoned").queued(id)
    }

    pub fn close(&self) {
        self.connection.close(0u32.into(), b"session closed");
        self.outgoing.lock().expect("outgoing poisoned").clear();
        self.receiver
            .lock()
            .expect("receiver poisoned")
            .disconnect();
    }
}

// A distinct fixed 16-byte frame, outside the JSON length domain; never an Offer.
const PROBE_TAG: [u8; 4] = *b"MYTP";
const PROBE_VERSION: u16 = 1;
const PROBE_PING: u16 = 1;
const PROBE_ECHO: u16 = 2;

async fn write_probe(send: &mut SendStream, kind: u16, nonce: u64) -> Result<()> {
    let mut bytes = [0; 16];
    bytes[..4].copy_from_slice(&PROBE_TAG);
    bytes[4..6].copy_from_slice(&PROBE_VERSION.to_be_bytes());
    bytes[6..8].copy_from_slice(&kind.to_be_bytes());
    bytes[8..].copy_from_slice(&nonce.to_be_bytes());
    send.write_all(&bytes).await?;
    Ok(())
}

async fn read_probe_body(receive: &mut RecvStream, kind: u16) -> Result<u64> {
    let mut bytes = [0; 12];
    receive.read_exact(&mut bytes).await?;
    ensure!(
        u16::from_be_bytes(bytes[..2].try_into()?) == PROBE_VERSION,
        "unsupported probe version"
    );
    ensure!(
        u16::from_be_bytes(bytes[2..4].try_into()?) == kind,
        "unexpected probe kind"
    );
    ensure!(
        receive.read_chunk(1, true).await?.is_none(),
        "trailing probe data"
    );
    Ok(u64::from_be_bytes(bytes[4..].try_into()?))
}

/// Bounded length-prefixed JSON followed by FIN. No arbitrary stream buffering.
pub async fn read_control<T: DeserializeOwned>(receive: &mut RecvStream) -> Result<T> {
    let mut prefix = [0; 4];
    receive.read_exact(&mut prefix).await?;
    read_control_body(receive, prefix).await
}

async fn read_control_body<T: DeserializeOwned>(
    receive: &mut RecvStream,
    prefix: [u8; 4],
) -> Result<T> {
    let size = u32::from_be_bytes(prefix) as usize;
    ensure!(
        size > 0 && size <= CONTROL_LIMIT,
        "oversize/empty control frame"
    );
    let mut bytes = vec![0; size];
    receive.read_exact(&mut bytes).await?;
    ensure!(
        receive.read_chunk(1, true).await?.is_none(),
        "trailing control data"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

pub async fn write_control<T: Serialize>(send: &mut SendStream, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(
        !bytes.is_empty() && bytes.len() <= CONTROL_LIMIT,
        "oversize/empty control frame"
    );
    send.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    send.write_all(&bytes).await?;
    Ok(())
}

#[cfg(test)]
#[path = "../tests/private/transport.rs"]
pub(crate) mod tests;
