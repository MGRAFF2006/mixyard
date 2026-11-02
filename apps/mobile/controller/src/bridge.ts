// No bearer, invitation, PCM or native route command crosses this interface.
export type Session = {
  deviceId: string; authorityId: string; revision: number; permissions: Permission[]
  backendAvailable: false; audioApplied: false
}
export type Permission = 'view' | 'control' | 'settings'
export type NativeCall = (method: 'session' | 'request' | 'disconnect', payload: unknown) => Promise<unknown>
export function encodeNativeMessage(method: string, payload: unknown, id: number) {
  if (method === 'request' && typeof payload !== 'string') throw new Error('Canonical control requests must remain raw JSON strings.')
  return JSON.stringify({ method, payload, id })
}
const MAX_BYTES = 2_097_152
function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid native control reply.')
  return value as Record<string, unknown>
}
export function readSession(value: unknown): Session {
  const v = record(value)
  if (typeof v.deviceId !== 'string' || !v.deviceId || typeof v.authorityId !== 'string' || !v.authorityId ||
      !Number.isSafeInteger(v.revision) || (v.revision as number) < 0 || !Array.isArray(v.permissions) ||
      v.permissions.some(p => !['view', 'control', 'settings'].includes(p)) || new Set(v.permissions).size !== v.permissions.length ||
      v.backendAvailable !== false || v.audioApplied !== false) throw new Error('Unsupported mobile control session. Reconnect explicitly.')
  return v as Session
}
function error(status: number, code: string, message: string) { return Response.json({ error: { code, message } }, { status }) }

// Api(fetcher) is the sole integration with the authoritative web console.
export class MobileBridge {
  private epoch = 0
  private active: Session | null = null
  private tail: Promise<void> = Promise.resolve()
  private queued = 0
  private notify: (value: Session | null) => void
  private call: NativeCall
  constructor(call: NativeCall, notify: (value: Session | null) => void = () => {}) { this.call = call; this.notify = notify }
  invalidate() { this.epoch++; this.active = null; this.notify(null) }
  async disconnect() { this.invalidate(); await this.call('disconnect', {}) }
  readonly fetch: typeof fetch = async (input, init) => {
    const path = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url
    if (!['/control/session', '/control/request'].includes(path)) return error(403, 'path-denied', 'Only native-owned control session/request are permitted.')
    if (init?.signal?.aborted) throw new DOMException('Cancelled', 'AbortError')
    if (path === '/control/session') {
      this.invalidate()
      const epoch = this.epoch
      const session = readSession(await this.call('session', {}))
      if (epoch !== this.epoch || init?.signal?.aborted) throw new Error('Mobile session changed. Refresh explicitly.')
      this.active = session; this.notify(session)
      return Response.json(session)
    }
    const session = this.active, epoch = this.epoch
    if (!session) return error(401, 'session-unavailable', 'Pair in native settings, then Refresh.')
    if (init?.method !== 'POST' || typeof init.body !== 'string' || new TextEncoder().encode(init.body).length > MAX_BYTES)
      return error(400, 'invalid-request', 'Expected bounded JSON POST.')
    if (this.queued >= 8) return error(409, 'request-in-flight', 'Native request budget exceeded. Retry explicitly.')
    const envelope = record(JSON.parse(init.body))
    if (envelope.deviceId !== session.deviceId || envelope.authorityId !== session.authorityId || envelope.revision !== session.revision ||
        typeof envelope.path !== 'string' || !/^\/api\/[a-z0-9/-]+$/.test(envelope.path) || envelope.path.includes('//') || !('body' in envelope))
      return error(409, 'identity-mismatch', 'Request must match the frozen native device/session/revision.')
    const previous = this.tail
    let release!: () => void
    this.tail = new Promise(resolve => { release = resolve }); this.queued++
    try {
      await previous
      if (epoch !== this.epoch || !this.active || this.active.revision !== envelope.revision || init.signal?.aborted)
        throw new Error('Mobile target/session changed before dispatch. No request replayed.')
      // Native rechecks this entire frozen envelope, then adds its own bearer.
      const native = record(await this.call('request', init.body))
      if (epoch !== this.epoch || init.signal?.aborted) throw new Error('Mobile target/session changed. Response discarded; no request replayed.')
      if (!Number.isInteger(native.status) || (native.status as number) < 200 || (native.status as number) > 599 || typeof native.json !== 'string' || new TextEncoder().encode(native.json).length > MAX_BYTES) throw new Error('Invalid native HTTP reply.')
      const reply = record(JSON.parse(native.json))
      if ('error' in reply) {
        const failure = record(reply.error)
        if (typeof failure.code !== 'string' || typeof failure.message !== 'string' || (native.status as number) < 400) throw new Error('Invalid native protocol error.')
        if (native.status === 401 || native.status === 409 || failure.code === 'invalid-origin') this.invalidate()
        return new Response(native.json, { status: native.status as number, headers: { 'Content-Type': 'application/json' } })
      }
      if (reply.deviceId !== session.deviceId || reply.authorityId !== session.authorityId ||
          !Number.isSafeInteger(reply.revision) || (reply.revision as number) < (this.active?.revision ?? session.revision) ||
          reply.status !== native.status || !('body' in reply)) {
        this.invalidate(); throw new Error('Invalid or stale mobile authority reply. Refresh explicitly.')
      }
      if ((native.status as number) >= 400) {
        const failure = record(record(reply.body).error)
        if (typeof failure.code !== 'string' || typeof failure.message !== 'string') throw new Error('Invalid canonical error reply.')
        if (native.status === 401 || native.status === 409 || failure.code === 'invalid-origin') {
          this.invalidate()
          return new Response(native.json, { status: native.status as number, headers: { 'Content-Type': 'application/json' } })
        }
      }
      this.active = { ...session, revision: reply.revision as number }; this.notify(this.active)
      return new Response(native.json, { status: native.status as number, headers: { 'Content-Type': 'application/json' } })
    } catch (cause) {
      if (epoch === this.epoch) this.invalidate()
      // Fetch-compatible transport failure clears the shared Api's authority;
      // structured HTTP permission denials above do not revoke other grants.
      throw new TypeError(cause instanceof Error ? cause.message : 'Native control transport failed; refresh before retrying. No request replayed.')
    }
    finally { this.queued--; release() }
  }
}
