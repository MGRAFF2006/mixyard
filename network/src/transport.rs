use crate::{Receiver, pcm::Outbox, protocol::*};
use anyhow::{Context, Result, ensure};
use quinn::{
    Connection, Endpoint, RecvStream, SendStream,
    crypto::rustls::{QuicClientConfig, QuicServerConfig},
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
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
            outgoing: tokio::sync::Mutex::new(Outbox::new(self.limits.packets_per_stream)),
            admission: tokio::sync::Mutex::new(()),
            running: AtomicBool::new(false),
            connection,
            permissions: self.permissions.clone(),
            limits: self.limits.clone(),
            _permit: permit,
        }))
    }
}

/// Network/codec worker boundary. No devices, mixing, resampling or realtime callbacks.
pub struct Session {
    connection: Connection,
    permissions: Permissions,
    limits: Limits,
    receiver: Mutex<Receiver>,
    outgoing: tokio::sync::Mutex<Outbox>,
    admission: tokio::sync::Mutex<()>,
    running: AtomicBool,
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
    /// At most one bounded control task runs; slow control does not stall datagrams.
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
        let mut controls = JoinSet::new();
        let result = loop {
            tokio::select! {
                incoming = self.connection.accept_bi() => match incoming {
                    Ok((mut send, mut receive)) => {
                        while let Some(result) = controls.try_join_next() {
                            if result.is_err() { self.close(); }
                        }
                        if !controls.is_empty() {
                            let _ = send.reset(2u32.into());
                            let _ = receive.stop(2u32.into());
                        } else {
                            let session = self.clone();
                            controls.spawn(async move {
                                let result = timeout(session.limits.control_timeout, async {
                                    let offer: Offer = read_control(&mut receive).await?;
                                    let admission = session.receiver.lock().expect("receiver poisoned").admit(offer);
                                    let reply = Reply { version: VERSION, accepted: admission.is_ok(), reason: admission.err().map_or_else(String::new, |e| e.to_string()) };
                                    write_control(&mut send, &reply).await?;
                                    send.finish()?;
                                    Ok::<_, anyhow::Error>(())
                                }).await;
                                if !matches!(result, Ok(Ok(()))) {
                                    // An uncertain admission cannot leave permission to send on this connection.
                                    session.close();
                                }
                            });
                        }
                    },
                    Err(e) => break Err(e.into()),
                },
                datagram = self.connection.read_datagram() => match datagram {
                    Ok(data) => self.receiver.lock().expect("receiver poisoned").ingest(&data),
                    Err(e) => break Err(e.into()),
                },
                Some(result) = controls.join_next(), if !controls.is_empty() => {
                    if let Err(e) = result { break Err(e.into()); }
                }
            }
        };
        controls.abort_all();
        self.receiver
            .lock()
            .expect("receiver poisoned")
            .disconnect();
        self.outgoing.lock().await.clear();
        result
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
                .await
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
                self.outgoing.lock().await.admit(offer);
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
        self.outgoing.lock().await.enqueue(id, payload)
    }

    pub async fn enqueue_pcm(&self, id: u64, pcm: &[u8]) -> Result<usize> {
        ensure!(
            self.connection.close_reason().is_none(),
            "disconnected: no replay/fallback"
        );
        self.outgoing.lock().await.enqueue_pcm(id, pcm)
    }

    /// A fair, nonblocking round. Caller paces by negotiated sample rate; this is not an audio clock.
    pub async fn flush_round(&self) -> Result<usize> {
        ensure!(self.connection.close_reason().is_none(), "disconnected");
        let mut outgoing = self.outgoing.lock().await;
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

    pub async fn sender_counters(&self, id: u64) -> Result<crate::Counters> {
        self.outgoing.lock().await.counters(id)
    }

    pub fn pull(&self, id: u64) -> Result<Vec<u8>> {
        let mut receiver = self.receiver.lock().expect("receiver poisoned");
        if self.connection.close_reason().is_some() {
            receiver.disconnect();
        }
        receiver.pull(id)
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
        self.receiver
            .lock()
            .expect("receiver poisoned")
            .disconnect();
    }
}

/// Bounded length-prefixed JSON followed by FIN. No arbitrary stream buffering.
pub async fn read_control<T: DeserializeOwned>(receive: &mut RecvStream) -> Result<T> {
    let mut prefix = [0; 4];
    receive.read_exact(&mut prefix).await?;
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
mod tests;
