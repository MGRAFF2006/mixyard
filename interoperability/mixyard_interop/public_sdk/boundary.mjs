// Browser bridge policy boundary. No SDK loading, discovery or network requests.
export const runtimeState = Object.freeze({ backendAvailable: false, audioApplied: false });
export const deny = () => false;

export function text(value, max = 256) {
  if (typeof value !== 'string' || !value.length || value.length > max || /[\x00-\x1f\x7f]/.test(value)) {
    throw new TypeError('Invalid bounded text');
  }
  return value;
}

export function endpoint(value) {
  if (value?.host !== '127.0.0.1' || !Number.isInteger(value.port) || value.port < 1 || value.port > 65535 || !['http', 'https'].includes(value.scheme ?? 'http')) {
    throw new TypeError('Only literal IPv4 loopback endpoints are admitted');
  }
  return Object.freeze({ host: value.host, port: value.port, scheme: value.scheme ?? 'http' });
}

export function mediaURL(value, target) {
  text(value, 2048);
  const base = `${target.scheme}://127.0.0.1:${target.port}`;
  // Check the raw authority too: URL normalizes integer, octal and shortened IPs.
  if (!value.startsWith(`${base}/`)) throw new TypeError('Media URL must use the exact pinned authority');
  const url = new URL(value);
  if (url.origin !== new URL(base).origin || url.username || url.password || url.hash) throw new TypeError('Invalid media URL');
  return url.href;
}

export function snapshot(value) {
  // Bound traversal before serialization, including caller-owned mutable metadata.
  let nodes = 0;
  function copy(item, depth) {
    if (++nodes > 512 || depth > 12) throw new TypeError('Metadata work budget exceeded');
    if (item === null || typeof item === 'boolean') return item;
    if (typeof item === 'string') {
      if (item.length > 4096) throw new TypeError('Metadata string exceeds budget');
      return item;
    }
    if (typeof item === 'number') {
      if (!Number.isFinite(item) || !Number.isSafeInteger(item)) throw new TypeError('Metadata requires exact safe integers');
      return item;
    }
    if (!item || typeof item !== 'object' || (!Array.isArray(item) && ![Object.prototype, null].includes(Object.getPrototypeOf(item)))) throw new TypeError('Only bounded JSON metadata is admitted');
    const keys = Object.keys(item);
    if (keys.length > 128 || (Array.isArray(item) && item.length > 128)) throw new TypeError('Metadata collection exceeds budget');
    const result = Array.isArray(item) ? [] : {};
    for (const key of keys) {
      if (key.length > 128 || key === '__proto__') throw new TypeError('Invalid metadata key');
      result[key] = copy(item[key], depth + 1);
    }
    return Object.freeze(result);
  }
  const result = copy(value, 0);
  if (new TextEncoder().encode(JSON.stringify(result)).length > 8192) throw new TypeError('Metadata exceeds 8 KiB');
  return result;
}

export class Boundary {
  #authorize;
  #busy = false;
  #cleanup;
  #closed = false;
  constructor({ authorize = deny, endpoint: target, identity, spec = null, timeoutMs = 5000 }, cleanup) {
    if (typeof authorize !== 'function') throw new TypeError('authorize must be synchronous');
    if (!Number.isInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 10000) throw new TypeError('Invalid operation timeout');
    this.context = Object.freeze({ endpoint: endpoint(target), identity: text(identity), spec: snapshot(spec) });
    this.timeoutMs = timeoutMs;
    this.#authorize = authorize;
    this.#cleanup = cleanup;
  }
  get closed() { return this.#closed; }
  check(role, action, details = {}) {
    if (this.#closed) throw new Error('Session closed');
    const context = Object.freeze({ ...this.context, ...details, action });
    try {
      if (this.#authorize(role, context) !== true) throw new Error('Grant denied or revoked');
      // Host authorization may synchronously dispatch revocation itself.
      if (this.#closed) throw new Error('Session closed during authorization');
    } catch (error) {
      this.close();
      throw error;
    }
    return context;
  }
  async run(operation) {
    if (this.#closed) throw new Error('Session closed');
    if (this.#busy) throw new Error('Operation already in flight; no queue');
    this.#busy = true;
    let timer;
    try {
      return await Promise.race([
        Promise.resolve().then(operation).then(value => {
          if (this.#closed) {
            this.#cleanup?.();
            throw new Error('Session closed during SDK operation');
          }
          return value;
        }, error => {
          // Native promises cannot be cancelled. Undo resources created by late completion.
          if (this.#closed) this.#cleanup?.();
          throw error;
        }),
        new Promise((_, reject) => { timer = setTimeout(() => {
          this.close();
          reject(new Error('SDK operation timed out; session closed'));
        }, this.timeoutMs); }),
      ]);
    } catch (error) {
      this.close();
      throw error;
    } finally {
      clearTimeout(timer);
      this.#busy = false;
    }
  }
  close() {
    if (this.#closed) return;
    this.#closed = true;
    this.#cleanup?.();
  }
}

export function volume(value) {
  if (!Number.isFinite(value) || value < 0 || value > 1) throw new TypeError('Volume must be 0..1');
  return value;
}
