import { Boundary, mediaURL, runtimeState, text, volume } from './boundary.mjs';

export function capabilities({ cast, chrome, Spotify, media, RTCPeerConnection, secureContext = false } = {}) {
  const result = (available, role, gate) => Object.freeze({ available: Boolean(available), role, gate, evidence: 'mocked-sdk-contracts-only', vendorNetworkExecuted: false, ...runtimeState });
  return Object.freeze({
    cast: result(cast?.framework?.CastContext && chrome?.cast?.media?.LoadRequest, 'media-playback/control', 'Injected official sender SDK, explicit selected session, compatible receiver and reachable approved media required'),
    airplay: result(typeof media?.webkitShowPlaybackTargetPicker === 'function', 'browser-media-control', 'Safari API, user gesture, selected media and eligible receiver required'),
    spotify: result(typeof Spotify?.Player === 'function', 'service-player/control', 'Injected SDK, eligible account, OAuth streaming scope, browser DRM/autoplay and Spotify policy required'),
    webrtc: result(secureContext && typeof RTCPeerConnection === 'function', 'cooperating-live-send/receive', 'Authenticated signaling, pinned identity/fingerprint, selected-track consent and explicit destination conversion required'),
    castReceiver: result(false, 'receiver/advertise', 'No certified desktop Cast receiver implementation'),
    airplayReceiver: result(false, 'receiver/advertise', 'No AirPlay/RAOP receiver implementation'),
    spotifyPCM: result(false, 'arbitrary-pcm', 'Spotify content is never exported, mixed or accepted as arbitrary PCM'),
  });
}

// Adopt a caller-selected official CAF CastSession; never autojoin or launch one.
export class CastSender {
  #session;
  #media;
  #policy;
  #sessionId;
  #loadedURL;
  constructor({ session, chrome, ...options }) {
    if (!session || typeof session.loadMedia !== 'function' || !chrome?.cast?.media?.MediaInfo || !chrome.cast.media.LoadRequest) throw new Error('Cast sender SDK/session unavailable');
    this.#session = session;
    this.#media = chrome.cast.media;
    this.#sessionId = text(session.getSessionId());
    if (session.getCastDevice()?.label !== options.identity) throw new Error('Cast receiver identity mismatch');
    this.#policy = new Boundary(options, () => {
      if (session.getSessionId() === this.#sessionId && session.getCastDevice()?.label === this.#policy.context.identity) session.endSession(true);
    });
  }
  #check(role, action, details = {}) {
    this.#policy.check(role, action, { sessionId: this.#sessionId, ...details });
    if (this.#session.getSessionId() !== this.#sessionId || this.#session.getCastDevice()?.label !== this.#policy.context.identity) {
      this.revoke();
      throw new Error('Cast session identity changed');
    }
  }
  async load({ url, contentType, noRedirectLease = false, conversionAccepted = false }) {
    const safeURL = mediaURL(url, this.#policy.context.endpoint);
    if (noRedirectLease !== true || conversionAccepted !== true) throw new Error('Immutable no-redirect media lease and destination conversion approval required');
    if (!['audio/wav', 'audio/flac', 'audio/ogg', 'audio/mpeg', 'audio/mp4'].includes(contentType)) throw new Error('Unsupported media content type');
    return this.#policy.run(async () => {
      this.#check('media-fetch', 'cast-load', { mediaURL: safeURL, contentType, noRedirectLease, conversionAccepted });
      this.#check('control', 'cast-load');
      const info = new this.#media.MediaInfo(safeURL, contentType);
      const request = new this.#media.LoadRequest(info);
      request.autoplay = false;
      const result = await this.#session.loadMedia(request);
      this.#check('media-fetch', 'cast-loaded', { mediaURL: safeURL, contentType });
      this.#loadedURL = safeURL;
      return result;
    });
  }
  async command(action, value) {
    if (!['play', 'pause', 'stop', 'volume'].includes(action)) throw new Error('Unsupported Cast command');
    if (action === 'volume') volume(value);
    return this.#policy.run(async () => {
      this.#check('control', `cast-${action}`);
      if (action === 'volume') {
        await this.#session.setVolume(value);
        this.#check('control', 'cast-volume-complete');
        return;
      }
      const media = this.#session.getMediaSession();
      if (!media) throw new Error('No selected Cast media session');
      if (action === 'play') {
        if (!this.#loadedURL || media.media?.contentId !== this.#loadedURL) throw new Error('Play requires the explicitly loaded media resource');
        this.#check('media-fetch', 'cast-play', { mediaURL: this.#loadedURL });
      }
      // Legacy Media methods are callback APIs even when CAF owns the session.
      await new Promise((resolve, reject) => media[action](null, resolve, reject));
      this.#check('control', `cast-${action}-complete`);
    });
  }
  revoke() { this.#policy.close(); }
}

// Does not assign src, start capture, choose a route or implement a receiver.
export class SafariAirPlay {
  #element;
  #policy;
  #source;
  #available = false;
  #availability;
  constructor({ media, ...options }) {
    if (typeof media?.webkitShowPlaybackTargetPicker !== 'function') throw new Error('Safari AirPlay API unavailable');
    this.#element = media;
    this.#source = mediaURL(media.currentSrc || media.src, options.endpoint);
    this.#policy = new Boundary(options, () => {
      media.pause();
      media.removeEventListener('webkitplaybacktargetavailabilitychanged', this.#availability);
    });
    this.#availability = event => { this.#available = event.availability === 'available'; };
    this.#policy.check('control', 'airplay-observe');
    media.addEventListener('webkitplaybacktargetavailabilitychanged', this.#availability);
  }
  get available() { return this.#available && !this.#policy.closed; }
  #check(action) {
    this.#policy.check('control', action);
    this.#policy.check('media-fetch', action, { mediaURL: this.#source });
    if ((this.#element.currentSrc || this.#element.src) !== this.#source) {
      this.revoke();
      throw new Error('Selected media changed');
    }
  }
  pick() {
    this.#check('airplay-picker');
    if (!this.#available) throw new Error('No AirPlay target available');
    // Synchronous call preserves the browser user-activation requirement.
    this.#element.webkitShowPlaybackTargetPicker();
  }
  async play() {
    return this.#policy.run(async () => {
      this.#check('airplay-play');
      await this.#element.play();
      this.#check('airplay-playing');
    });
  }
  pause() { this.#check('airplay-pause'); this.#element.pause(); }
  setVolume(value) { volume(value); this.#check('airplay-volume'); this.#element.volume = value; }
  revoke() { this.#policy.close(); }
}

export class SpotifyPlayer {
  #player;
  #policy;
  #device = null;
  #listeners = [];
  #connected = false;
  #tokenPending = false;
  #tokenTimer;
  #tokenRequests = 0;
  constructor({ Spotify, getOAuthToken, name = 'Mixyard service player', ...options }) {
    if (typeof Spotify?.Player !== 'function' || typeof getOAuthToken !== 'function') throw new Error('Spotify SDK/token provider unavailable');
    text(name, 100);
    this.#policy = new Boundary(options, () => {
      clearTimeout(this.#tokenTimer);
      this.#tokenPending = false;
      if (!this.#player) return;
      this.#player.disconnect();
      for (const [event, callback] of this.#listeners) this.#player.removeListener(event, callback);
      this.#device = null;
      this.#connected = false;
    });
    this.#policy.check('service-control', 'spotify-create');
    this.#player = new Spotify.Player({
      name, volume: 0, enableMediaSession: false,
      getOAuthToken: callback => {
        this.#policy.check('service-control', 'spotify-token-request');
        if (this.#tokenPending || ++this.#tokenRequests > 8) {
          this.revoke();
          throw new Error('Spotify token request budget exceeded');
        }
        this.#tokenPending = true;
        this.#tokenTimer = setTimeout(() => this.revoke(), this.#policy.timeoutMs);
        try {
          getOAuthToken(token => {
            try {
              this.#policy.check('service-control', 'spotify-token-delivery');
              if (!this.#tokenPending) { this.revoke(); throw new Error('Duplicate token delivery'); }
              this.#tokenPending = false;
              clearTimeout(this.#tokenTimer);
              callback(text(token, 4096));
            } catch (error) { this.revoke(); throw error; }
          });
        } catch (error) { this.revoke(); throw error; }
      },
    });
    const listen = (event, callback) => {
      this.#listeners.push([event, callback]);
      this.#policy.check('service-control', 'spotify-listen');
      try {
        if (this.#player.addListener(event, callback) !== true) throw new Error('Spotify listener registration failed');
      } catch (error) { this.revoke(); throw error; }
    };
    listen('ready', ({ device_id }) => {
      try {
        this.#policy.check('advertise', 'spotify-ready');
        this.#policy.check('service-control', 'spotify-ready');
        this.#device = text(device_id);
      } catch { this.revoke(); }
    });
    listen('not_ready', () => { this.#device = null; });
    for (const event of ['initialization_error', 'authentication_error', 'account_error', 'playback_error']) listen(event, () => this.revoke());
  }
  get deviceId() { return this.#device; }
  async connect() {
    if (this.#connected) throw new Error('Spotify player already connected');
    return this.#policy.run(async () => {
      this.#policy.check('advertise', 'spotify-connect');
      this.#policy.check('service-control', 'spotify-connect');
      if (await this.#player.connect() !== true) throw new Error('Spotify connection failed');
      this.#policy.check('service-control', 'spotify-connected');
      this.#connected = true;
      return true;
    });
  }
  async command(action, value) {
    if (!['pause', 'resume', 'nextTrack', 'previousTrack', 'seek', 'setVolume', 'activateElement'].includes(action)) throw new Error('Unsupported Spotify service command');
    if (action === 'setVolume') volume(value);
    if (action === 'seek' && (!Number.isSafeInteger(value) || value < 0 || value > 86400000)) throw new Error('Invalid seek position');
    return this.#policy.run(async () => {
      this.#policy.check('service-control', `spotify-${action}`, { deviceId: this.#device });
      if (!this.#connected || !this.#device) throw new Error('Spotify device not ready');
      const result = await (value === undefined ? this.#player[action]() : this.#player[action](value));
      this.#policy.check('service-control', `spotify-${action}-complete`, { deviceId: this.#device });
      return result;
    });
  }
  revoke() { this.#policy.close(); }
}
