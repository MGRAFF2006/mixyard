import { Boundary, snapshot, text } from './boundary.mjs';

const MAX_SIGNAL = 32768;
const MAX_CANDIDATES = 64;
const FINGERPRINT = /^(?:[0-9A-F]{2}:){31}[0-9A-F]{2}$/;
const encoder = new TextEncoder();

function fingerprint(value) {
  if (typeof value !== 'string' || !FINGERPRINT.test(value)) throw new TypeError('SHA-256 DTLS fingerprint required');
  return value;
}

export function validateCandidate(candidate, ports) {
  if (!candidate || typeof candidate !== 'object') throw new TypeError('Invalid ICE candidate');
  const raw = text(candidate.candidate, 1024);
  const fields = raw.split(' ');
  if (fields.length !== 8 || !/^candidate:[A-Za-z0-9+/]{1,32}$/.test(fields[0]) || fields[1] !== '1' || fields[2].toLowerCase() !== 'udp' || !/^\d{1,10}$/.test(fields[3]) || fields[4] !== '127.0.0.1' || !/^\d{1,5}$/.test(fields[5]) || !ports.includes(Number(fields[5])) || fields[6] !== 'typ' || fields[7] !== 'host') {
    throw new Error('Only pinned literal-loopback UDP host candidates are admitted');
  }
  if (candidate.sdpMid !== '0' || candidate.sdpMLineIndex !== 0) throw new Error('Only the selected audio m-line is admitted');
  return { candidate: raw, sdpMid: '0', sdpMLineIndex: 0 };
}

export function validateDescription(description, { remoteFingerprint, ports, codec }) {
  if (!description || !['offer', 'answer'].includes(description.type) || typeof description.sdp !== 'string' || description.sdp.length > 24576 || encoder.encode(description.sdp).length > 24576) throw new Error('Invalid bounded SDP');
  const lines = description.sdp.split('\r\n');
  if (lines.length > 256 || lines.some(line => line.length > 2048 || /[\x00-\x1f\x7f]/.test(line))) throw new Error('Invalid SDP line bounds');
  const media = lines.filter(line => line.startsWith('m='));
  if (media.length !== 1 || !/^m=audio (?:9|\d{1,5}) UDP\/TLS\/RTP\/SAVPF \d{1,3}$/.test(media[0])) throw new Error('One encrypted audio codec/m-line required');
  const mediaPort = Number(media[0].split(' ')[1]);
  if (mediaPort !== 9 && !ports.includes(mediaPort)) throw new Error('Unpinned SDP media port');
  const payload = media[0].split(' ').at(-1);
  const maps = lines.filter(line => line.startsWith('a=rtpmap:'));
  const format = `${codec.mimeType.slice(6)}/${codec.clockRate}/${codec.channels}`.toLowerCase();
  if (maps.length !== 1 || maps[0].toLowerCase() !== `a=rtpmap:${payload} ${format}`) throw new Error('SDP codec differs from the explicit destination conversion');
  if (!lines.includes('a=rtcp-mux') || !lines.includes('a=mid:0') || lines.some(line => line.startsWith('a=crypto:') || line.startsWith('a=ice-server:'))) throw new Error('Invalid secure audio profile');
  const fps = lines.filter(line => line.startsWith('a=fingerprint:'));
  if (fps.length !== 1 || !fps[0].startsWith('a=fingerprint:sha-256 ')) throw new Error('DTLS fingerprint required');
  const fp = fingerprint(fps[0].slice('a=fingerprint:sha-256 '.length));
  if (remoteFingerprint && fp !== remoteFingerprint) throw new Error('Peer DTLS fingerprint mismatch');
  if (!lines.some(line => /^a=ice-ufrag:[A-Za-z0-9+/]{4,256}$/.test(line)) || !lines.some(line => /^a=ice-pwd:[A-Za-z0-9+/]{22,256}$/.test(line))) throw new Error('ICE credentials required');
  let candidates = 0;
  for (const line of lines) {
    if (line.startsWith('c=') && !['c=IN IP4 0.0.0.0', 'c=IN IP4 127.0.0.1'].includes(line)) throw new Error('Non-loopback SDP connection');
    if (line.startsWith('o=') && !/^o=\S+ \d+ \d+ IN IP4 (?:127\.0\.0\.1|0\.0\.0\.0)$/.test(line)) throw new Error('Non-loopback SDP origin');
    if (line.startsWith('a=rtcp:') && !/^a=rtcp:(?:9|\d{1,5}) IN IP4 (?:127\.0\.0\.1|0\.0\.0\.0)$/.test(line)) throw new Error('Non-loopback RTCP');
    if (line.startsWith('a=candidate:')) {
      if (++candidates > MAX_CANDIDATES) throw new Error('Description candidate budget exceeded');
      validateCandidate({ candidate: line.slice(2), sdpMid: '0', sdpMLineIndex: 0 }, ports);
    }
    if (line.startsWith('a=remote-candidates:')) throw new Error('Remote-candidates extension unavailable');
  }
  return { type: description.type, sdp: description.sdp };
}

// Restricted cooperating-browser profile. No capture, playback, data channels,
// external ICE servers, fetch, WebSocket or discovery is performed here.
export class WebRTCAudio {
  #pc;
  #policy;
  #role;
  #sendSignal;
  #onTrack;
  #local;
  #remote;
  #timer;
  #config;
  #remoteSet = false;
  #incoming = 0;
  #outgoing = 0;
  #messages = 0;
  #remoteTrackId;
  #selectedTrackId;
  #supportedCodecs;
  constructor({ RTCPeerConnection, RTCRtpSender, secureContext = false, role, localIdentity, sessionId, remoteFingerprint, ports, codec, conversionAccepted = false, sendSignal, onTrack = () => {}, lifetimeMs = 30000, ...options }) {
    if (!secureContext || typeof RTCPeerConnection !== 'function') throw new Error('Secure browser WebRTC unavailable');
    if (!['live-send', 'live-receive'].includes(role)) throw new Error('Choose one directional role per peer connection');
    if (!Array.isArray(ports) || !ports.length || ports.length > 4 || ports.some(port => !Number.isInteger(port) || port < 1 || port > 65535)) throw new Error('Pin 1..4 owned loopback media ports');
    if (typeof sendSignal !== 'function' || typeof onTrack !== 'function') throw new TypeError('Injected signaling and track callback required');
    if (!Number.isInteger(lifetimeMs) || lifetimeMs < 1 || lifetimeMs > 60000) throw new Error('Session lifetime exceeds profile');
    if (conversionAccepted !== true || codec?.mimeType !== 'audio/opus' || codec.clockRate !== 48000 || codec.channels !== 2) throw new Error('Explicit destination-local Opus/48000/2 conversion required');
    const supported = RTCRtpSender?.getCapabilities('audio')?.codecs?.filter(item => item.mimeType.toLowerCase() === 'audio/opus' && item.clockRate === 48000 && item.channels === 2);
    if (!supported?.length) throw new Error('Requested browser codec unavailable');
    this.#config = Object.freeze({ localIdentity: text(localIdentity), sessionId: text(sessionId), remoteFingerprint: fingerprint(remoteFingerprint), ports: Object.freeze([...ports]), codec: snapshot(codec) });
    this.#role = role;
    this.#sendSignal = sendSignal;
    this.#onTrack = onTrack;
    this.#policy = new Boundary(options, () => {
      clearTimeout(this.#timer);
      this.#local?.stop();
      this.#remote?.stop();
      if (this.#pc) {
        this.#pc.onicecandidate = null;
        this.#pc.ontrack = null;
        this.#pc.close();
      }
    });
    this.#check('control', 'webrtc-create');
    this.#check(role, 'webrtc-create');
    this.#pc = new RTCPeerConnection({ iceServers: [], bundlePolicy: 'max-bundle', rtcpMuxPolicy: 'require', iceCandidatePoolSize: 0 });
    this.#pc.onicecandidate = event => {
      if (!event.candidate) return;
      try {
        if (++this.#outgoing > MAX_CANDIDATES) throw new Error('Outgoing candidate budget exceeded');
        const candidate = validateCandidate(event.candidate, this.#config.ports);
        this.#emit({ candidate });
      } catch { this.revoke(); }
    };
    this.#pc.ontrack = event => {
      try {
        this.#check('live-receive', 'webrtc-track');
        if (this.#role !== 'live-receive' || this.#remote || event.track.kind !== 'audio' || event.track.id !== this.#remoteTrackId) throw new Error('Unselected remote track');
        this.#remote = event.track;
        this.#onTrack(event.track, this.#policy.context);
      } catch {
        event.track.stop();
        this.revoke();
      }
    };
    // Avoid keeping Node contract tests alive; browsers return a number here.
    this.#timer = setTimeout(() => this.revoke(), lifetimeMs);
    this.#timer.unref?.();
    this.#supportedCodecs = supported.map(item => ({ ...item }));
  }
  #check(role, action) {
    return this.#policy.check(role, action, { sessionId: this.#config.sessionId, trackId: this.#selectedTrackId ?? this.#remoteTrackId ?? null, wireTrackId: this.#local?.id ?? this.#remoteTrackId ?? null, conversion: this.#config.codec });
  }
  #description(value, incoming) {
    const description = validateDescription(value, { ...this.#config,
      remoteFingerprint: incoming ? this.#config.remoteFingerprint : null });
    const count = description.sdp.split('\r\n').filter(line => line.startsWith('a=candidate:')).length;
    if (incoming) {
      if (this.#incoming + count > MAX_CANDIDATES) throw new Error('Incoming candidate budget exceeded');
      this.#incoming += count;
    } else {
      if (this.#outgoing + count > MAX_CANDIDATES) throw new Error('Outgoing candidate budget exceeded');
      this.#outgoing += count;
    }
    return description;
  }
  selectTrack(track) {
    if (this.#role !== 'live-send' || this.#local || track?.kind !== 'audio' || track.readyState !== 'live' || typeof track.clone !== 'function') throw new Error('Select one live audio track');
    this.#selectedTrackId = text(track.id);
    this.#policy.check('live-send', 'webrtc-select-track', { trackId: this.#selectedTrackId });
    this.#local = track.clone();
    try {
      this.#check('live-send', 'webrtc-add-track');
      const transceiver = this.#pc.addTransceiver(this.#local, { direction: 'sendonly' });
      this.#check('live-send', 'webrtc-codec');
      transceiver.setCodecPreferences(this.#supportedCodecs);
    } catch (error) { this.revoke(); throw error; }
    return this.#local.id;
  }
  selectRemoteTrack(trackId) {
    if (this.#role !== 'live-receive' || this.#remoteTrackId) throw new Error('Select one remote audio track');
    this.#remoteTrackId = text(trackId);
    try {
      this.#check('live-receive', 'webrtc-select-remote-track');
      const transceiver = this.#pc.addTransceiver('audio', { direction: 'recvonly' });
      this.#check('live-receive', 'webrtc-codec');
      transceiver.setCodecPreferences(this.#supportedCodecs);
    } catch (error) { this.revoke(); throw error; }
  }
  #emit(body) {
    this.#check(this.#role, 'webrtc-signal-send');
    this.#check('control', 'webrtc-signal-send');
    const payload = JSON.stringify({ v: 1, session: this.#config.sessionId, from: this.#config.localIdentity, to: this.#policy.context.identity, ...body });
    if (encoder.encode(payload).length > MAX_SIGNAL) throw new Error('Signal exceeds budget');
    // A sync, bounded handoff only: the injected transport owns its own IO budget.
    if (this.#sendSignal(payload) !== true) throw new Error('Signaling backpressure or unavailable transport');
  }
  async offer() {
    if (!this.#local && !this.#remoteTrackId) throw new Error('Selected-track consent required');
    return this.#policy.run(async () => {
      this.#check(this.#role, 'webrtc-offer');
      const offer = this.#description(await this.#pc.createOffer(), false);
      this.#check(this.#role, 'webrtc-set-local');
      await this.#pc.setLocalDescription(offer);
      this.#emit({ description: offer });
    });
  }
  async accept(raw) {
    if (typeof raw !== 'string' || raw.length > MAX_SIGNAL || encoder.encode(raw).length > MAX_SIGNAL) throw new Error('Signal exceeds byte budget');
    if (!this.#local && !this.#remoteTrackId) throw new Error('Selected-track consent required');
    if (++this.#messages > 68) { this.revoke(); throw new Error('Signaling message budget exceeded'); }
    let message;
    try {
      message = JSON.parse(raw);
      if (!message || message.v !== 1 || message.session !== this.#config.sessionId || message.from !== this.#policy.context.identity || message.to !== this.#config.localIdentity || Object.keys(message).sort().join(',') !== (message.description ? 'description,from,session,to,v' : 'candidate,from,session,to,v')) throw new Error('Signaling identity/envelope mismatch');
    } catch (error) { this.revoke(); throw error; }
    return this.#policy.run(async () => {
      this.#check('control', 'webrtc-signal-receive');
      this.#check(this.#role, 'webrtc-signal-receive');
      if (message.description) {
        if (this.#remoteSet) throw new Error('Renegotiation requires a new consented session');
        const description = this.#description(message.description, true);
        this.#check(this.#role, 'webrtc-set-remote');
        await this.#pc.setRemoteDescription(description);
        this.#remoteSet = true;
        if (description.type === 'offer') {
          this.#check(this.#role, 'webrtc-answer');
          const answer = this.#description(await this.#pc.createAnswer(), false);
          this.#check(this.#role, 'webrtc-set-local');
          await this.#pc.setLocalDescription(answer);
          this.#emit({ description: answer });
        }
      } else {
        if (!this.#remoteSet || ++this.#incoming > MAX_CANDIDATES) throw new Error('Candidate ordering/budget rejected; no queue');
        const candidate = validateCandidate(message.candidate, this.#config.ports);
        this.#check(this.#role, 'webrtc-add-candidate');
        await this.#pc.addIceCandidate(candidate);
      }
    });
  }
  revoke() { this.#policy.close(); }
}
