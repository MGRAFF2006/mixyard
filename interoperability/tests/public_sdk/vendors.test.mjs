import test from 'node:test';
import assert from 'node:assert/strict';
import { CastSender, SafariAirPlay, SpotifyPlayer, capabilities, runtimeState } from '../../mixyard_interop/public_sdk/index.mjs';

const endpoint = { host: '127.0.0.1', port: 45678, scheme: 'http' };
const spec = { stream: 1, name: 'Selected source', codec: 'raw-f64-le', sample_rate: 96000, channels: ['FR', 'FL'], frames_per_packet: 32, provenance: { sources: [{ peer: 'origin', source: 'premix', groups: ['private-approved'] }], visited_peers: ['origin'] } };
const options = { endpoint, identity: 'selected-device', spec };

function castFixture(authorize = () => true, extra = {}, policy = {}) {
  const calls = [];
  const media = Object.fromEntries(['play', 'pause', 'stop'].map(action => [action, (request, done) => { calls.push(action); done(); }]));
  let sessionId = 'owned-session';
  media.media = { contentId: 'http://127.0.0.1:45678/owned.wav' };
  const session = {
    getSessionId: () => sessionId, getCastDevice: () => ({ label: 'selected-device' }),
    loadMedia: async request => { calls.push(request); },
    getMediaSession: () => media, setVolume: async value => { calls.push(['volume', value]); },
    endSession: value => { calls.push(['end', value]); }, ...extra,
  };
  const chrome = { cast: { media: { MediaInfo: class { constructor(url, type) { this.contentId = url; this.contentType = type; } }, LoadRequest: class { constructor(media) { this.media = media; } } } } };
  return { sender: new CastSender({ ...options, authorize, session, chrome, ...policy }), calls, changeSession: () => { sessionId = 'unrelated'; } };
}
const resource = { url: 'http://127.0.0.1:45678/owned.wav', contentType: 'audio/wav', noRedirectLease: true, conversionAccepted: true };

test('Cast invokes official media constructors, callback controls and volume with exact immutable source metadata', async () => {
  const seen = [];
  const { sender, calls } = castFixture((role, context) => { seen.push([role, context]); return true; });
  await sender.load(resource);
  await sender.command('play');
  await sender.command('pause');
  await sender.command('stop');
  await sender.command('volume', 0.25);
  assert.equal(calls[0].media.contentId, resource.url);
  assert.equal(calls[0].autoplay, false);
  assert.deepEqual(calls.slice(1), ['play', 'pause', 'stop', ['volume', 0.25]]);
  assert.deepEqual(seen[0][1].spec, spec);
  assert(Object.isFrozen(seen[0][1].spec.channels));
  assert(seen.some(([role]) => role === 'media-fetch'));
  sender.revoke();
});

test('Cast denies revoked grants and session switching; no operation targets a replacement', async () => {
  let permitted = true;
  const { sender, calls } = castFixture(() => permitted);
  await sender.load(resource);
  permitted = false;
  await assert.rejects(sender.command('play'), /denied/);
  assert.deepEqual(calls.at(-1), ['end', true]);
  assert(!calls.includes('play'));
  const second = castFixture();
  second.changeSession();
  await assert.rejects(second.sender.command('play'), /identity changed/);
  assert(!second.calls.includes('play'));
});

test('media handoff rejects DNS, redirects without lease, noncanonical IP, and absent conversion', async () => {
  const { sender, calls } = castFixture();
  for (const url of ['http://localhost:45678/a', 'http://2130706433:45678/a', 'http://127.0.0.2:45678/a', 'http://127.0.0.1:45679/a', 'http://127.0.0.1:45678@evil.test/a']) {
    await assert.rejects(sender.load({ ...resource, url }));
  }
  await assert.rejects(sender.load({ ...resource, noRedirectLease: false }));
  await assert.rejects(sender.load({ ...resource, conversionAccepted: false }));
  assert.equal(calls.length, 0);
  sender.revoke();
});

test('Cast timeout closes session, rejects queued work and never retries', async () => {
  const { sender, calls } = castFixture(() => true, { loadMedia: () => new Promise(() => {}) }, { timeoutMs: 15 });
  const pending = sender.load(resource);
  await assert.rejects(sender.command('play'), /in flight/);
  await assert.rejects(pending, /timed out/);
  await assert.rejects(sender.command('play'), /closed/);
  assert.deepEqual(calls, [['end', true]]);
});

function safari(authorize = () => true) {
  const callbacks = new Map();
  const calls = [];
  const media = { src: resource.url, currentSrc: resource.url, volume: 0,
    addEventListener: (event, fn) => callbacks.set(event, fn), removeEventListener: event => callbacks.delete(event),
    webkitShowPlaybackTargetPicker: () => calls.push('picker'), play: async () => calls.push('play'), pause: () => calls.push('pause') };
  return { adapter: new SafariAirPlay({ ...options, authorize, media }), media, calls, callbacks };
}

test('Safari availability gates synchronous route picker; lifecycle removes observer and pauses only owned media', async () => {
  let allowed = true;
  const f = safari(() => allowed);
  assert.throws(() => f.adapter.pick(), /No AirPlay/);
  f.callbacks.get('webkitplaybacktargetavailabilitychanged')({ availability: 'available' });
  assert.equal(f.adapter.available, true);
  f.adapter.pick();
  await f.adapter.play();
  f.adapter.setVolume(0.4);
  assert.equal(f.media.volume, 0.4);
  allowed = false;
  assert.throws(() => f.adapter.pick(), /denied/);
  assert.deepEqual(f.calls, ['picker', 'play', 'pause']);
  assert.equal(f.callbacks.size, 0);
});

test('Safari source replacement is refused before playback', async () => {
  const f = safari();
  f.media.currentSrc = 'http://127.0.0.1:45678/other.wav';
  await assert.rejects(f.adapter.play(), /Selected media changed/);
  assert(!f.calls.includes('play'));
});

function spotify(authorize = () => true, overrides = {}) {
  const calls = [];
  const listeners = new Map();
  let config;
  const Spotify = { Player: class {
    constructor(value) { config = value; calls.push('construct'); }
    addListener(event, callback) { listeners.set(event, callback); return true; }
    removeListener(event) { listeners.delete(event); }
    connect() { calls.push('connect'); return Promise.resolve(true); }
    disconnect() { calls.push('disconnect'); }
    pause() { calls.push('pause'); return Promise.resolve(); }
    resume() { calls.push('resume'); return Promise.resolve(); }
    setVolume(value) { calls.push(['volume', value]); return Promise.resolve(); }
    seek(value) { calls.push(['seek', value]); return Promise.resolve(); }
    nextTrack() { calls.push('next'); return Promise.resolve(); }
    previousTrack() { calls.push('previous'); return Promise.resolve(); }
    activateElement() { calls.push('activate'); return Promise.resolve(); }
  } };
  const adapter = new SpotifyPlayer({ ...options, authorize, Spotify, getOAuthToken: callback => callback('ephemeral-token'), ...overrides });
  return { adapter, calls, listeners, config };
}

test('Spotify invokes service-only SDK and delivers tokens only through injected callback', async () => {
  const seen = [];
  const f = spotify((role, context) => { seen.push([role, context.action]); return true; });
  assert.equal(f.config.volume, 0);
  assert.equal(f.config.enableMediaSession, false);
  let token;
  f.config.getOAuthToken(value => { token = value; });
  assert.equal(token, 'ephemeral-token');
  await f.adapter.connect();
  f.listeners.get('ready')({ device_id: 'spotify-owned-device' });
  assert.equal(f.adapter.deviceId, 'spotify-owned-device');
  for (const command of ['activateElement', 'resume', 'pause', 'nextTrack', 'previousTrack']) await f.adapter.command(command);
  await f.adapter.command('seek', 1200);
  await f.adapter.command('setVolume', 0.2);
  assert(f.calls.includes('resume'));
  assert(f.calls.includes('next'));
  assert(seen.every(([role]) => ['service-control', 'advertise'].includes(role)));
  f.adapter.revoke();
  assert.equal(f.listeners.size, 0);
});

test('Spotify token delivery, connection and later control recheck independent live roles', async () => {
  let allowed = true;
  let deliver;
  const f = spotify(() => allowed, { getOAuthToken: callback => { deliver = callback; } });
  f.config.getOAuthToken(() => assert.fail('revoked token delivered'));
  allowed = false;
  assert.throws(() => deliver('ephemeral-token'), /denied/);
  assert(f.calls.includes('disconnect'));
  assert(!f.calls.includes('connect'));
  const onlyControl = spotify(role => role === 'service-control');
  await assert.rejects(onlyControl.adapter.connect(), /denied/);
  assert(!onlyControl.calls.includes('connect'));
});

test('SDK absence and default authorization fail closed without capability inflation', async () => {
  const available = capabilities();
  assert.equal(available.cast.available, false);
  assert.equal(available.airplayReceiver.available, false);
  assert.equal(available.spotifyPCM.available, false);
  assert.deepEqual(runtimeState, { backendAvailable: false, audioApplied: false });
  const f = castFixture(undefined, {}, { authorize: undefined });
  await assert.rejects(f.sender.load(resource), /denied/);
  const denied = castFixture(() => false);
  await assert.rejects(denied.sender.load(resource), /denied/);
});

test('whole-premix exclusion denies Cast media handoff before any SDK media call', async () => {
  const f = castFixture((role, context) => {
    assert.deepEqual(context.spec.provenance, spec.provenance);
    if (context.spec.provenance.sources.some(source => source.groups.includes('private-approved'))) return false;
    return true;
  });
  await assert.rejects(f.sender.load(resource), /denied/);
  assert(f.calls.length >= 1);
  assert(f.calls.every(call => Array.isArray(call) && call[0] === 'end'));
});

test('revocation during pending Cast load cleans late completion and cannot start playback', async () => {
  let finish;
  const f = castFixture(() => true, { loadMedia: () => new Promise(resolve => { finish = resolve; }) });
  const pending = f.sender.load(resource);
  await Promise.resolve();
  f.sender.revoke();
  finish();
  await assert.rejects(pending, /closed/);
  assert(f.calls.filter(item => item[0] === 'end').length >= 2);
  assert(!f.calls.includes('play'));
});

test('pending Spotify connect cannot restore readiness or listeners after revoke; late native completion disconnects again', async () => {
  let finish;
  const calls = [];
  const listeners = new Map();
  const SDK = { Player: class {
    addListener(event, fn) { listeners.set(event, fn); return true; }
    removeListener(event) { listeners.delete(event); }
    connect() { calls.push('connect'); return new Promise(resolve => { finish = resolve; }); }
    disconnect() { calls.push('disconnect'); }
  } };
  const adapter = new SpotifyPlayer({ ...options, authorize: () => true, Spotify: SDK, getOAuthToken: () => {} });
  const pending = adapter.connect();
  await Promise.resolve();
  const lateReady = listeners.get('ready');
  adapter.revoke();
  lateReady({ device_id: 'late-device' });
  finish(true);
  await assert.rejects(pending, /closed/);
  assert.equal(adapter.deviceId, null);
  assert.equal(listeners.size, 0);
  assert.deepEqual(calls, ['connect', 'disconnect', 'disconnect']);
});

test('pending SDK operation retains a copied, deeply frozen endpoint/spec/peer', async () => {
  let finish;
  const seen = [];
  const mutable = { endpoint: { ...endpoint }, identity: 'selected-device', spec: structuredClone(spec) };
  const f = castFixture((role, context) => { seen.push(context); return true; }, {
    loadMedia: () => new Promise(resolve => { finish = resolve; }),
  }, mutable);
  const pending = f.sender.load(resource);
  await Promise.resolve();
  mutable.endpoint.port = 1;
  mutable.identity = 'other-device';
  mutable.spec.sample_rate = 8000;
  mutable.spec.channels.reverse();
  mutable.spec.provenance.sources[0].groups.push('excluded');
  finish();
  await pending;
  assert.equal(seen.at(-1).endpoint.port, 45678);
  assert.equal(seen.at(-1).identity, 'selected-device');
  assert.deepEqual(seen.at(-1).spec, spec);
  assert.throws(() => { seen[0].spec.channels.reverse(); }, TypeError);
  f.sender.revoke();
});

test('Cast management control cannot start media without an independent fetch grant', async () => {
  const f = castFixture(role => role === 'control');
  await f.sender.command('pause');
  await assert.rejects(f.sender.command('play'), /explicitly loaded/);
  assert(!f.calls.includes('play'));
});
