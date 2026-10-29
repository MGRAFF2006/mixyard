import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { WebRTCAudio } from '../../mixyard_interop/public_sdk/index.mjs';
import { validateCandidate, validateDescription } from '../../mixyard_interop/public_sdk/webrtc.mjs';

const localFP = Array(32).fill('AA').join(':');
const remoteFP = Array(32).fill('BB').join(':');
const codec = { mimeType: 'audio/opus', clockRate: 48000, channels: 2 };
const candidate = { candidate: 'candidate:1 1 udp 2122260223 127.0.0.1 45679 typ host', sdpMid: '0', sdpMLineIndex: 0 };
const spec = { stream: 2, name: 'Raw source', codec: 'raw-f64-le', sample_rate: 192000, channels: ['R', 'L'], frames_per_packet: 32, provenance: { sources: [{ peer: 'origin', source: 'premix', groups: ['approved-source'] }], visited_peers: ['origin', 'upstream-peer'] } };
function sdp(fp = localFP, type = 'offer') {
  return { type, sdp: [
    'v=0', 'o=- 1 1 IN IP4 127.0.0.1', 's=-', 't=0 0', 'a=group:BUNDLE 0',
    'm=audio 9 UDP/TLS/RTP/SAVPF 111', 'c=IN IP4 0.0.0.0', 'a=mid:0', 'a=rtcp-mux',
    'a=ice-ufrag:abcd', 'a=ice-pwd:abcdefghijklmnopqrstuv', `a=fingerprint:sha-256 ${fp}`,
    'a=setup:actpass', 'a=rtpmap:111 opus/48000/2', 'a=sendrecv', '',
  ].join('\r\n') };
}
function track(id = 'approved-track') {
  return { id, kind: 'audio', readyState: 'live', stopped: false,
    stop() { this.stopped = true; }, clone() { return track(`${id}-owned`); } };
}
function fixture({ role = 'live-send', authorize = () => true, ...overrides } = {}) {
  let pc;
  const calls = [];
  const sent = [];
  const received = [];
  const contexts = [];
  class Peer {
    constructor(config) { pc = this; calls.push(['create', config]); }
    addTransceiver(selected, config) { calls.push(['transceiver', selected, config]); return { setCodecPreferences: codecs => calls.push(['codec', codecs]) }; }
    async createOffer() { calls.push('offer'); return sdp(); }
    async createAnswer() { calls.push('answer'); return sdp(localFP, 'answer'); }
    async setLocalDescription(value) { calls.push(['local', value]); }
    async setRemoteDescription(value) { calls.push(['remote', value]); }
    async addIceCandidate(value) { calls.push(['candidate', value]); }
    close() { calls.push('close'); }
  }
  const adapter = new WebRTCAudio({ endpoint: { host: '127.0.0.1', port: 45678 }, identity: 'remote', spec,
    localIdentity: 'local', sessionId: 'consented-session', remoteFingerprint: remoteFP, ports: [45679], codec,
    conversionAccepted: true, secureContext: true, RTCPeerConnection: Peer,
    RTCRtpSender: { getCapabilities: () => ({ codecs: [codec, { mimeType: 'audio/PCMU', clockRate: 8000, channels: 1 }] }) },
    sendSignal: raw => { sent.push(JSON.parse(raw)); return true; }, onTrack: selected => received.push(selected),
    authorize: (grant, context) => { contexts.push([grant, context]); return authorize(grant, context); }, role, ...overrides });
  return { adapter, calls, sent, received, contexts, get pc() { return pc; } };
}
function envelope(body) { return JSON.stringify({ v: 1, session: 'consented-session', from: 'remote', to: 'local', ...body }); }

test('WebRTC sends only a selected owned clone with explicit codec and unchanged original precision/provenance', async () => {
  const f = fixture();
  const original = track();
  assert.equal(f.adapter.selectTrack(original), 'approved-track-owned');
  await f.adapter.offer();
  assert.deepEqual(f.calls[0][1], { iceServers: [], bundlePolicy: 'max-bundle', rtcpMuxPolicy: 'require', iceCandidatePoolSize: 0 });
  assert.deepEqual(f.calls.find(item => item[0] === 'codec')[1], [codec]);
  assert.equal(f.sent[0].description.type, 'offer');
  assert.deepEqual(f.contexts.at(-1)[1].spec, spec);
  assert.deepEqual(f.contexts.at(-1)[1].conversion, codec);
  f.pc.onicecandidate({ candidate });
  assert.deepEqual(f.sent[1].candidate, candidate);
  f.adapter.revoke();
  assert.equal(original.stopped, false);
  assert.equal(f.calls.find(item => item[0] === 'transceiver')[1].stopped, true);
  assert.equal(f.pc.onicecandidate, null);
});

test('WebRTC receive authenticates signaling/fingerprint and admits only the selected track without playback', async () => {
  const f = fixture({ role: 'live-receive' });
  f.adapter.selectRemoteTrack('peer-selected-track');
  await f.adapter.accept(envelope({ description: sdp(remoteFP) }));
  assert(f.calls.includes('answer'));
  assert.equal(f.sent[0].description.type, 'answer');
  const selected = track('peer-selected-track');
  f.pc.ontrack({ track: selected });
  assert.deepEqual(f.received, [selected]);
  await f.adapter.accept(envelope({ candidate }));
  assert(f.calls.some(item => item[0] === 'candidate'));
  f.adapter.revoke();
  assert.equal(selected.stopped, true);
});

test('identity/fingerprint tampering cannot reach setRemoteDescription', async () => {
  for (const raw of [envelope({ description: sdp(localFP) }), envelope({ description: sdp(remoteFP) }).replace('"from":"remote"', '"from":"attacker"')]) {
    const f = fixture();
    f.adapter.selectTrack(track());
    await assert.rejects(f.adapter.accept(raw), /mismatch/);
    assert(!f.calls.some(item => item[0] === 'remote'));
    assert(f.calls.includes('close'));
  }
});

test('candidate and SDP policy rejects DNS, LAN, relay, extra channels, insecure media and oversized input', () => {
  for (const value of ['localhost', '127.1', '192.168.1.2', 'peer.local', '::1']) {
    assert.throws(() => validateCandidate({ ...candidate, candidate: candidate.candidate.replace('127.0.0.1', value) }, [45679]));
  }
  assert.throws(() => validateCandidate({ ...candidate, candidate: candidate.candidate.replace('45679', '45680') }, [45679]));
  assert.throws(() => validateCandidate({ ...candidate, candidate: candidate.candidate.replace('typ host', 'typ relay') }, [45679]));
  for (const value of [sdp().sdp.replace('UDP/TLS/RTP/SAVPF', 'RTP/AVP'), sdp().sdp.replace('opus/48000/2', 'opus/48000/1'), sdp().sdp.replace('c=IN IP4 0.0.0.0', 'c=IN IP4 evil.test'), `${sdp().sdp}m=video 9 UDP/TLS/RTP/SAVPF 96\r\n`, 'x'.repeat(24577)]) {
    assert.throws(() => validateDescription({ type: 'offer', sdp: value }, { ports: [45679], codec }));
  }
});

test('receive authority cannot send, unexpected track closes, and revocation blocks further native effects', async () => {
  let allowed = true;
  const f = fixture({ role: 'live-receive', authorize: () => allowed });
  assert.throws(() => f.adapter.selectTrack(track()), /Select/);
  f.adapter.selectRemoteTrack('expected');
  const wrong = track('wrong');
  f.pc.ontrack({ track: wrong });
  assert.equal(wrong.stopped, true);
  assert.equal(f.received.length, 0);
  const sender = fixture({ authorize: () => allowed });
  sender.adapter.selectTrack(track());
  allowed = false;
  await assert.rejects(sender.adapter.offer(), /denied/);
  assert(!sender.calls.includes('offer'));
});

test('bounded candidate work has no pre-description queue and closes on candidate overflow', async () => {
  const before = fixture();
  before.adapter.selectTrack(track());
  await assert.rejects(before.adapter.accept(envelope({ candidate })), /ordering/);
  assert(!before.calls.some(item => item[0] === 'candidate'));
  const f = fixture();
  f.adapter.selectTrack(track());
  await f.adapter.accept(envelope({ description: sdp(remoteFP, 'answer') }));
  for (let i = 0; i < 64; i++) await f.adapter.accept(envelope({ candidate }));
  await assert.rejects(f.adapter.accept(envelope({ candidate })), /budget/);
  assert.equal(f.calls.filter(item => item[0] === 'candidate').length, 64);
});

test('authorization is checked after asynchronous offer generation before setting local SDP', async () => {
  let allowed = true;
  const f = fixture({ authorize: () => allowed });
  f.adapter.selectTrack(track());
  f.pc.createOffer = async () => { allowed = false; return sdp(); };
  await assert.rejects(f.adapter.offer(), /denied/);
  assert(!f.calls.some(item => item[0] === 'local'));
});

test('real owned loopback HTTP delivers bounded signaling into production adapter; no browser/media qualification', async () => {
  const f = fixture({ role: 'live-receive' });
  f.adapter.selectRemoteTrack('selected');
  const server = createServer((request, response) => {
    request.setTimeout(1000, () => request.destroy());
    const chunks = [];
    let size = 0;
    request.on('data', chunk => {
      size += chunk.length;
      if (size > 32768) { request.destroy(); return; }
      chunks.push(chunk);
    });
    request.on('end', async () => {
      try { await f.adapter.accept(Buffer.concat(chunks).toString('utf8')); response.writeHead(204).end(); }
      catch { response.writeHead(400).end(); }
    });
  });
  server.requestTimeout = 1000;
  server.headersTimeout = 1000;
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  try {
    const response = await fetch(`http://127.0.0.1:${server.address().port}/signal`, {
      method: 'POST', body: envelope({ description: sdp(remoteFP) }), redirect: 'error', signal: AbortSignal.timeout(1000),
    });
    assert.equal(response.status, 204);
    assert(f.calls.some(item => item[0] === 'remote'));
    assert.equal(f.sent[0].description.type, 'answer');
  } finally {
    f.adapter.revoke();
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
});

test('revocation while SDP generation is pending prevents late local SDP, signal, or track effects', async () => {
  let finish;
  const f = fixture();
  const original = track();
  f.adapter.selectTrack(original);
  f.pc.createOffer = () => new Promise(resolve => { finish = resolve; });
  const pending = f.adapter.offer();
  await Promise.resolve();
  const lateTrack = f.pc.ontrack;
  f.adapter.revoke();
  finish(sdp());
  await assert.rejects(pending, /closed/);
  assert(!f.calls.some(item => item[0] === 'local'));
  assert.equal(f.sent.length, 0);
  const unexpected = track('late');
  lateTrack({ track: unexpected });
  assert.equal(unexpected.stopped, true);
  assert.equal(original.stopped, false);
});

test('exclusion callback sees full premix provenance and denies before peer construction', () => {
  let nativeCalls = 0;
  assert.throws(() => fixture({ authorize: (role, context) => !context.spec.provenance.sources.some(source => source.groups.includes('approved-source')),
    RTCPeerConnection: class { constructor() { nativeCalls++; } },
  }), /denied/);
  assert.equal(nativeCalls, 0);
});

test('signaling backpressure closes owned tracks rather than buffering an offer', async () => {
  const f = fixture({ sendSignal: () => false });
  f.adapter.selectTrack(track());
  await assert.rejects(f.adapter.offer(), /backpressure/);
  assert(f.calls.includes('close'));
  assert.equal(f.calls.find(item => item[0] === 'transceiver')[1].stopped, true);
});

test('oversized signal and unsupported runtime/codec reject without native signaling effects', async () => {
  const f = fixture();
  f.adapter.selectTrack(track());
  await assert.rejects(f.adapter.accept('x'.repeat(32769)), /budget/);
  assert(!f.calls.some(item => item[0] === 'remote'));
  assert(f.calls.includes('close'));
  assert.equal(f.calls.find(item => item[0] === 'transceiver')[1].stopped, true);
  await assert.rejects(f.adapter.offer(), /closed/);
  assert.equal(f.sent.length, 0);
  assert.throws(() => fixture({ secureContext: false }), /unavailable/);
  assert.throws(() => fixture({ conversionAccepted: false }), /conversion/);
  assert.throws(() => fixture({ codec: { ...codec, channels: 1 } }), /conversion/);
});

test('invalid signaling type or oversized UTF-8 also revokes the selected session', async () => {
  for (const raw of [null, {}, 'é'.repeat(20000)]) {
    const f = fixture();
    const original = track();
    f.adapter.selectTrack(original);
    await assert.rejects(f.adapter.accept(raw), /budget/);
    assert(f.calls.includes('close'));
    assert.equal(f.calls.find(item => item[0] === 'transceiver')[1].stopped, true);
    assert.equal(original.stopped, false);
    await assert.rejects(f.adapter.offer(), /closed/);
    assert.equal(f.sent.length, 0);
  }
});

test('revocation during selected-track admission prevents cloning or native track effects', () => {
  let adapter;
  let cloned = 0;
  const f = fixture({ authorize: (role, context) => {
    if (context.action === 'webrtc-select-track') adapter.revoke();
    return true;
  } });
  adapter = f.adapter;
  const original = track();
  original.clone = () => { cloned++; return track('owned'); };
  assert.throws(() => adapter.selectTrack(original), /closed/);
  assert.equal(cloned, 0);
  assert.equal(original.stopped, false);
  assert(!f.calls.some(item => item[0] === 'transceiver'));
  assert(f.calls.includes('close'));
});

function embedded(description, count) {
  return { ...description, sdp: description.sdp + Array.from({ length: count }, (_, i) =>
    `a=candidate:${i + 1} 1 udp 2122260223 127.0.0.1 45679 typ host\r\n`).join('') };
}

test('oversized embedded remote or local candidates reject before native SDP handoff', async () => {
  const incoming = fixture({ role: 'live-receive' });
  incoming.adapter.selectRemoteTrack('approved');
  await assert.rejects(incoming.adapter.accept(envelope({ description: embedded(sdp(remoteFP, 'answer'), 65) })), /candidate budget/);
  assert(!incoming.calls.some(item => item[0] === 'remote'));
  const outgoing = fixture();
  outgoing.adapter.selectTrack(track());
  outgoing.pc.createOffer = async () => embedded(sdp(), 65);
  await assert.rejects(outgoing.adapter.offer(), /candidate budget/);
  assert(!outgoing.calls.some(item => item[0] === 'local'));
  assert.equal(outgoing.sent.length, 0);
});

test('remote SDP and trickle consume one combined incoming candidate budget', async () => {
  const f = fixture({ role: 'live-receive' });
  f.adapter.selectRemoteTrack('approved');
  await f.adapter.accept(envelope({ description: embedded(sdp(remoteFP, 'answer'), 63) }));
  await f.adapter.accept(envelope({ candidate }));
  await assert.rejects(f.adapter.accept(envelope({ candidate })), /budget/);
  assert.equal(f.calls.filter(item => item[0] === 'candidate').length, 1);
  assert(f.calls.includes('close'));
});

test('local SDP and trickle consume one combined outgoing candidate budget', async () => {
  const f = fixture();
  f.adapter.selectTrack(track());
  f.pc.createOffer = async () => embedded(sdp(), 63);
  await f.adapter.offer();
  const dispatch = f.pc.onicecandidate;
  dispatch({ candidate });
  dispatch({ candidate });
  assert.equal(f.sent.filter(item => item.candidate).length, 1);
  assert(f.calls.includes('close'));
});

test('reentrant selected-track authorization preserves one owned source and matching context', () => {
  let adapter;
  let innerAttempted = false;
  const clones = [];
  const selected = id => ({ ...track(id), clone() {
    const owned = track(`${id}-owned`);
    clones.push(owned);
    return owned;
  } });
  const f = fixture({ authorize: (_role, context) => {
    if (context.action === 'webrtc-select-track' && !innerAttempted) {
      innerAttempted = true;
      assert.throws(() => adapter.selectTrack(selected('B')), /Select/);
    }
    return true;
  } });
  adapter = f.adapter;
  assert.equal(adapter.selectTrack(selected('A')), 'A-owned');
  assert.deepEqual(clones.map(item => item.id), ['A-owned']);
  assert.equal(f.calls.filter(item => item[0] === 'transceiver').length, 1);
  for (const [, context] of f.contexts.filter(([, c]) => ['webrtc-add-track', 'webrtc-codec'].includes(c.action))) {
    assert.equal(context.trackId, 'A');
    assert.equal(context.wireTrackId, 'A-owned');
  }
  adapter.revoke();
  assert(clones.every(item => item.stopped));
});

test('uncaught reentrant selection closes before acquiring any clone', () => {
  let adapter;
  let attempted = false;
  let clones = 0;
  const original = { ...track('A'), clone() { clones++; return track('A-owned'); } };
  const f = fixture({ authorize: (_role, context) => {
    if (context.action === 'webrtc-select-track' && !attempted) {
      attempted = true;
      adapter.selectTrack(track('B'));
    }
    return true;
  } });
  adapter = f.adapter;
  assert.throws(() => adapter.selectTrack(original), /Select/);
  assert.equal(clones, 0);
  assert.equal(f.calls.filter(item => item[0] === 'transceiver').length, 0);
  assert(f.calls.includes('close'));
});

test('revocation inside cloning stops the late owned clone before native handoff', () => {
  const f = fixture();
  const owned = track('A-owned');
  const original = { ...track('A'), clone() { f.adapter.revoke(); return owned; } };
  assert.throws(() => f.adapter.selectTrack(original), /closed/);
  assert.equal(owned.stopped, true);
  assert.equal(original.stopped, false);
  assert.equal(f.calls.filter(item => item[0] === 'transceiver').length, 0);
});
