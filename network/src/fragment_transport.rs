//! Explicit, authenticated raw-block framing alongside the unchanged v1 path.
//! Worker-only; returned blocks retain their exact canonical Spec and provenance.
use crate::fragmentation::{self, Block, Config, Descriptor, Fragmenter, Reassembler};
use crate::protocol::Reply;
use crate::transport::{SecureEndpoint, read_control, write_control};
use crate::{Identity, Limits, Offer, PairedPeer, Permissions, Session, Spec, endpoint};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
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
    sync::{Mutex as AsyncMutex, Notify},
    time::timeout,
};

const VERSION: u16 = 2;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockOffer {
    version: u16,
    generation: u64,
    spec: Spec,
    max_datagram: usize,
}

fn descriptor(offer: &Offer) -> Result<Descriptor> {
    ensure!(
        offer.generation > 0 && offer.version == crate::VERSION,
        "invalid canonical offer"
    );
    Ok(Descriptor {
        stream: offer.spec.stream,
        generation: offer.generation,
        block_bytes: offer.spec.payload_bytes()?,
        frames_per_block: u64::from(offer.spec.frames_per_packet),
    })
}

fn validate_grants(permissions: &Permissions, limits: &Limits, config: &Config) -> Result<()> {
    limits.validate()?;
    config.validate()?;
    // Reuse the existing identity validation without imposing v1 packet sizes.
    let mut identities = permissions.clone();
    identities.send.clear();
    identities.receive.clear();
    identities.validate(limits)?;
    ensure!(
        config.max_streams <= limits.max_streams
            && config.window <= limits.packets_per_stream
            && config.max_datagram <= limits.max_datagram,
        "fragment budgets exceed endpoint limits"
    );
    for (grants, sender, receiver) in [
        (
            &permissions.send,
            &permissions.local_peer,
            &permissions.remote_peer,
        ),
        (
            &permissions.receive,
            &permissions.remote_peer,
            &permissions.local_peer,
        ),
    ] {
        ensure!(grants.len() <= config.max_streams, "too many block grants");
        let mut reservation = Reassembler::new(config.clone())?;
        let mut rate = 0u128;
        for (id, spec) in grants {
            ensure!(*id == spec.stream, "permission key mismatch");
            spec.validate_descriptor(sender, receiver)?;
            let offer = Offer {
                version: crate::VERSION,
                generation: 1,
                spec: spec.clone(),
            };
            reservation.admit(descriptor(&offer)?)?;
            let payload = spec.payload_bytes()? as u128;
            let chunks = payload.div_ceil((config.max_datagram - fragmentation::HEADER) as u128);
            rate = rate
                .checked_add(
                    (payload + chunks * fragmentation::HEADER as u128)
                        .checked_mul(u128::from(spec.sample_rate))
                        .context("block wire rate overflow")?
                        .div_ceil(u128::from(spec.frames_per_packet)),
                )
                .context("aggregate block rate overflow")?;
        }
        ensure!(
            rate <= u128::from(limits.max_bytes_per_second),
            "fragmented wire rate exceeds admission budget"
        );
    }
    for spec in permissions.receive.values() {
        ensure!(
            !spec
                .provenance
                .sources
                .iter()
                .flat_map(|s| &s.groups)
                .any(|g| permissions.excluded_receive_groups.contains(g)),
            "excluded source in premix"
        );
    }
    Ok(())
}

/// Opt-in factory. Audio grants are separate from TLS identity and any control roles.
/// The underlying v1 receiver is private and has no audio permissions. Version 2
/// is agreed on the reliable control exchange before any block can be sent.
pub fn fragment_endpoint(
    bind: SocketAddr,
    identity: Identity,
    peer: PairedPeer,
    permissions: Permissions,
    limits: Limits,
    config: Config,
) -> Result<FragmentEndpoint> {
    validate_grants(&permissions, &limits, &config)?;
    let mut empty = permissions.clone();
    empty.send.clear();
    empty.receive.clear();
    let inner = endpoint(bind, identity, peer, empty, limits.clone())?;
    Ok(FragmentEndpoint {
        inner,
        permissions,
        limits,
        config,
    })
}

pub struct FragmentEndpoint {
    inner: SecureEndpoint,
    permissions: Permissions,
    limits: Limits,
    config: Config,
}
impl FragmentEndpoint {
    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.inner.local_addr()
    }
    pub async fn connect(&self, addr: SocketAddr) -> Result<Arc<FragmentSession>> {
        self.session(self.inner.connect(addr).await?)
    }
    pub async fn accept(&self) -> Result<Arc<FragmentSession>> {
        self.session(self.inner.accept().await?)
    }
    fn session(&self, base: Arc<Session>) -> Result<Arc<FragmentSession>> {
        ensure!(
            base.connection
                .max_datagram_size()
                .is_some_and(|n| n >= self.config.max_datagram),
            "path cannot carry exact negotiated fragment size"
        );
        Ok(Arc::new(FragmentSession {
            base,
            permissions: self.permissions.clone(),
            limits: self.limits.clone(),
            config: self.config.clone(),
            receiver: Mutex::new(ReceiveState {
                reassembler: Reassembler::new(self.config.clone())?,
                revoked: BTreeSet::new(),
            }),
            outgoing: Mutex::new(SendState::default()),
            admission: AsyncMutex::new(()),
            changed: Notify::new(),
            running: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        }))
    }
}
struct ReceiveState {
    reassembler: Reassembler,
    revoked: BTreeSet<u64>,
}
#[derive(Default)]
struct SendState {
    streams: BTreeMap<u64, Sending>,
    revoked: BTreeSet<u64>,
}
struct Sending {
    offer: Offer,
    next: u64,
    busy: bool,
    active: bool,
}

pub struct FragmentSession {
    base: Arc<Session>,
    permissions: Permissions,
    limits: Limits,
    config: Config,
    receiver: Mutex<ReceiveState>,
    outgoing: Mutex<SendState>,
    admission: AsyncMutex<()>,
    changed: Notify,
    running: AtomicBool,
    closed: AtomicBool,
}
struct CloseOnDrop(Arc<FragmentSession>);
impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0.close();
    }
}
struct NegotiationGuard {
    session: Arc<FragmentSession>,
    confirmed: bool,
}
impl Drop for NegotiationGuard {
    fn drop(&mut self) {
        if !self.confirmed {
            self.session.close();
        }
    }
}
struct SendingGuard {
    session: Arc<FragmentSession>,
    stream: u64,
    generation: u64,
    committed: bool,
}
impl Drop for SendingGuard {
    fn drop(&mut self) {
        if let Some(state) = self
            .session
            .outgoing
            .lock()
            .expect("send poisoned")
            .streams
            .get_mut(&self.stream)
            && state.offer.generation == self.generation
        {
            state.busy = false;
            if !self.committed {
                state.active = false;
            }
        }
    }
}

impl FragmentSession {
    fn connected(&self) -> Result<()> {
        ensure!(
            !self.closed.load(Ordering::Acquire) && self.base.connection.close_reason().is_none(),
            "fragment session closed"
        );
        Ok(())
    }
    pub async fn run(self: Arc<Self>) -> Result<()> {
        ensure!(
            !self.running.swap(true, Ordering::AcqRel),
            "block worker already running"
        );
        let _guard = CloseOnDrop(self.clone());
        let mut control: Option<Pin<Box<dyn Future<Output = Result<()>> + Send>>> = None;
        let mut timer = tokio::time::interval(
            self.config
                .timeout
                .clamp(Duration::from_millis(1), Duration::from_millis(100)),
        );
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut window = Instant::now();
        let mut controls = 0usize;
        loop {
            tokio::select! {
                request = self.base.connection.accept_bi(), if control.is_none() => {
                    let (mut send, mut receive) = request?;
                    if window.elapsed() >= Duration::from_secs(1) { window = Instant::now(); controls = 0; }
                    controls += 1;
                    ensure!(controls <= self.limits.max_streams.saturating_add(16), "block control rate exceeded");
                    let owner = self.clone();
                    control = Some(Box::pin(async move {
                        timeout(owner.limits.control_timeout, async {
                            let request: BlockOffer = read_control(&mut receive).await?;
                            let accepted = owner.admit(request);
                            let reply = Reply { version: VERSION, accepted: accepted.is_ok(), reason: accepted.err().map_or_else(String::new, |e| e.to_string()) };
                            write_control(&mut send, &reply).await?;
                            send.finish()?;
                            Ok::<_, anyhow::Error>(())
                        }).await.context("block control deadline")?
                    }));
                },
                datagram = self.base.connection.read_datagram() => {
                    let bytes = datagram?;
                    let result = self.receiver.lock().expect("receive poisoned").reassembler.ingest(&bytes, Instant::now());
                    if matches!(result, Ok(fragmentation::Outcome::Ready { .. })) { self.changed.notify_waiters(); }
                    // Malformed/unadmitted datagrams cannot mutate grants or stop siblings.
                },
                result = async { control.as_mut().expect("control active").await }, if control.is_some() => {
                    control = None;
                    result?;
                },
                _ = timer.tick() => {
                    self.receiver.lock().expect("receive poisoned").reassembler.expire(Instant::now())?;
                    self.changed.notify_waiters();
                }
            }
        }
    }
    fn admit(&self, wire: BlockOffer) -> Result<()> {
        self.connected()?;
        ensure!(
            wire.version == VERSION && wire.max_datagram == self.config.max_datagram,
            "fragment version/size mismatch"
        );
        ensure!(
            self.permissions.receive.get(&wire.spec.stream) == Some(&wire.spec),
            "block descriptor/provenance not permitted"
        );
        let mut rx = self.receiver.lock().expect("receive poisoned");
        ensure!(
            !rx.revoked.contains(&wire.spec.stream),
            "receive permission revoked"
        );
        rx.reassembler.admit(descriptor(&Offer {
            version: crate::VERSION,
            generation: wire.generation,
            spec: wire.spec,
        })?)?;
        self.changed.notify_waiters();
        Ok(())
    }
    pub async fn negotiate(self: &Arc<Self>, offer: Offer) -> Result<()> {
        self.connected()?;
        descriptor(&offer)?;
        ensure!(
            self.permissions.send.get(&offer.spec.stream) == Some(&offer.spec),
            "send descriptor/provenance not permitted"
        );
        let _admission = self.admission.lock().await;
        {
            let tx = self.outgoing.lock().expect("send poisoned");
            ensure!(
                !tx.revoked.contains(&offer.spec.stream),
                "send permission revoked"
            );
            ensure!(
                tx.streams
                    .get(&offer.spec.stream)
                    .is_none_or(|s| !s.busy && offer.generation > s.offer.generation),
                "generation busy/stale"
            );
        }
        let mut uncertain = NegotiationGuard {
            session: self.clone(),
            confirmed: false,
        };
        let wire = BlockOffer {
            version: VERSION,
            generation: offer.generation,
            spec: offer.spec.clone(),
            max_datagram: self.config.max_datagram,
        };
        timeout(self.limits.control_timeout, async {
            let (mut send, mut receive) = self.base.connection.open_bi().await?;
            write_control(&mut send, &wire).await?;
            send.finish()?;
            let reply: Reply = read_control(&mut receive).await?;
            ensure!(
                reply.version == VERSION && reply.accepted,
                "block offer rejected: {}",
                reply.reason
            );
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("block negotiation timeout")??;
        self.connected()?;
        let mut tx = self.outgoing.lock().expect("send poisoned");
        ensure!(
            !tx.revoked.contains(&offer.spec.stream),
            "send permission revoked during negotiation"
        );
        tx.streams.insert(
            offer.spec.stream,
            Sending {
                offer,
                next: 0,
                busy: false,
                active: true,
            },
        );
        // Successful reliable agreement owns the new generation. Cancel/error closes
        // the uncertain session; never assume that only one peer applied an offer.
        uncertain.confirmed = true;
        Ok(())
    }
    /// At most one bounded borrowed block is in flight per stream. No shared FIFO.
    /// Awaiting QUIC capacity avoids evicting a queued sibling datagram. Caller
    /// paces blocks; completion is local enqueue, never physical delivery proof.
    pub async fn send(self: &Arc<Self>, stream: u64, generation: u64, bytes: &[u8]) -> Result<()> {
        self.connected()?;
        let (offer, sequence) = {
            let mut tx = self.outgoing.lock().expect("send poisoned");
            ensure!(!tx.revoked.contains(&stream), "send permission revoked");
            let state = tx
                .streams
                .get_mut(&stream)
                .context("unadmitted block sender")?;
            ensure!(
                state.active && !state.busy && state.offer.generation == generation,
                "inactive/busy/stale block sender"
            );
            ensure!(
                bytes.len() == state.offer.spec.payload_bytes()?,
                "block payload length mismatch"
            );
            let next = state
                .next
                .checked_add(1)
                .context("block sequence exhausted")?;
            state.busy = true;
            let result = (state.offer.clone(), state.next);
            state.next = next;
            result
        };
        let mut guard = SendingGuard {
            session: self.clone(),
            stream,
            generation,
            committed: false,
        };
        let fragmenter = Fragmenter::new(self.config.clone(), descriptor(&offer)?)?;
        timeout(self.limits.control_timeout, async {
            for fragment in fragmenter.fragments(sequence, bytes)? {
                self.connected()?;
                {
                    let tx = self.outgoing.lock().expect("send poisoned");
                    ensure!(
                        tx.streams
                            .get(&stream)
                            .is_some_and(|s| s.active && s.offer.generation == generation),
                        "block send invalidated"
                    );
                }
                self.base
                    .connection
                    .send_datagram_wait(fragment.into())
                    .await?;
                // Bound work per poll even when an entire large block fits the
                // empty transport queue; independent streams must get a turn.
                tokio::task::yield_now().await;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("block send timeout")??;
        guard.committed = true;
        Ok(())
    }
    /// Exact complete block only. Missing/expired data is an error/None: a host
    /// consumer chooses explicit silence and maintains its own local sample clock.
    pub fn take(
        &self,
        stream: u64,
        generation: u64,
        sequence: u64,
    ) -> Result<Option<(Spec, Block)>> {
        self.connected()?;
        let mut rx = self.receiver.lock().expect("receive poisoned");
        ensure!(!rx.revoked.contains(&stream), "receive permission revoked");
        let spec = self
            .permissions
            .receive
            .get(&stream)
            .context("ungranted receiver")?;
        Ok(rx
            .reassembler
            .take(stream, generation, sequence, Instant::now())?
            .map(|block| (spec.clone(), block)))
    }
    pub async fn receive(
        &self,
        stream: u64,
        generation: u64,
        sequence: u64,
    ) -> Result<(Spec, Block)> {
        timeout(self.limits.control_timeout, async {
            loop {
                let notification = self.changed.notified();
                tokio::pin!(notification);
                notification.as_mut().enable();
                if let Some(block) = self.take(stream, generation, sequence)? {
                    return Ok(block);
                }
                notification.await;
            }
        })
        .await
        .context("block receive timeout")?
    }
    pub fn advance(&self, stream: u64, generation: u64, sequence: u64) -> Result<()> {
        self.connected()?;
        self.receiver
            .lock()
            .expect("receive poisoned")
            .reassembler
            .advance(stream, generation, sequence)?;
        Ok(())
    }
    pub fn revoke_receive(&self, stream: u64) -> Result<()> {
        ensure!(
            self.permissions.receive.contains_key(&stream),
            "unknown receive permission"
        );
        let mut rx = self.receiver.lock().expect("receive poisoned");
        rx.revoked.insert(stream);
        match rx.reassembler.revoke(stream) {
            Ok(()) | Err(fragmentation::Error::Unadmitted) => {}
            Err(e) => return Err(e.into()),
        }
        self.changed.notify_waiters();
        Ok(())
    }
    pub fn revoke_send(&self, stream: u64) -> Result<()> {
        ensure!(
            self.permissions.send.contains_key(&stream),
            "unknown send permission"
        );
        let mut tx = self.outgoing.lock().expect("send poisoned");
        tx.revoked.insert(stream);
        if let Some(s) = tx.streams.get_mut(&stream) {
            s.active = false;
        }
        Ok(())
    }
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.base.close();
        self.receiver
            .lock()
            .expect("receive poisoned")
            .reassembler
            .disconnect();
        self.outgoing.lock().expect("send poisoned").streams.clear();
        self.changed.notify_waiters();
    }
}
impl Drop for FragmentSession {
    fn drop(&mut self) {
        self.base.close();
    }
}
