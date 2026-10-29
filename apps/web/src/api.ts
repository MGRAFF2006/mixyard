import { dspJson } from './dsp-json.ts'
import { object, string } from './model.ts'
import { parseAutomationJson } from './situations.ts'

export type Permission = 'view' | 'control' | 'settings'
export type DeviceIdentity = { deviceId: string; authorityId: string; revision: number; permissions: Permission[]; backendAvailable: false; audioApplied: false }
export type SessionState = { deviceId: string | null; authorityId: string | null; revision: number | null; displayName: string; connected: boolean; epoch: number }
export type ControlConnection = { deviceId: string; credential: string }
export class StaleSessionError extends Error {
  constructor() { super('Device session changed. Old completion discarded; refresh and review before another action.') }
}
function identity(value: unknown): DeviceIdentity {
  const v = object(value)
  if (!string(v.deviceId) || !string(v.authorityId) || !Number.isSafeInteger(v.revision) || (v.revision as number) < 0 ||
    !Array.isArray(v.permissions) || v.permissions.some(p => !['view', 'control', 'settings'].includes(p)) ||
    new Set(v.permissions).size !== v.permissions.length || v.backendAvailable !== false || v.audioApplied !== false) throw new Error('Unsupported device session identity or capabilities.')
  return { deviceId: v.deviceId as string, authorityId: v.authorityId as string, revision: v.revision as number, permissions: [...v.permissions] as Permission[], backendAvailable: false, audioApplied: false }
}

export class ApiError extends Error {
  status: number
  code: string
  constructor(status: number, code: string, message: string) { super(message); this.status = status; this.code = code }
}
export class Api {
  token: string | null = null
  identity: DeviceIdentity | null = null
  epoch = 0
  connectionError = ''
  private fetcher: typeof fetch
  private control: Readonly<ControlConnection> | null
  private requests = new Set<AbortController>()
  constructor(fetcher: typeof fetch = (input, init) => fetch(input, init), control: ControlConnection | null = null) {
    this.fetcher = fetcher; this.control = control ? Object.freeze({ ...control }) : null
  }
  // The injected transport owns endpoint TLS trust/pinning. Relative paths only;
  // the browser's default local CSRF transport never acquires bearer authority.
  static forControl(fetcher: typeof fetch, connection: ControlConnection) {
    if (!connection.deviceId || !connection.credential) throw Error('A verified device identity and controller credential are required.')
    return new Api(fetcher, connection)
  }
  get mode() { return this.control ? 'control' : 'local' }
  get connected() { return this.control ? this.identity !== null : this.token !== null }
  assertEpoch(epoch: number) { if (epoch !== this.epoch) throw new StaleSessionError() }
  disconnect(reason = '') {
    this.connectionError = reason; this.token = null; this.identity = null; this.epoch++
    for (const controller of this.requests) controller.abort()
    this.requests.clear()
  }
  async connect() {
    this.disconnect()
    const epoch = this.epoch
    if (this.control) {
      const next = identity(await this.request('/control/session', {}, true))
      this.assertEpoch(epoch)
      if (next.deviceId !== this.control.deviceId) { this.disconnect('Wrong device identity. Verify the paired endpoint.'); throw Error(this.connectionError) }
      this.identity = next
      return
    }
    const v = object(await this.request('/api/session'))
    this.assertEpoch(epoch)
    if (v.apiVersion !== 1 || v.audioAvailable !== false) throw new Error('Unsupported API session. Reload after updating the service.')
    this.token = string(v.csrfToken)
  }
  async post(path: string, body: unknown = {}): Promise<unknown> {
    if (!/^\/api\/[a-z0-9/-]+$/.test(path)) throw Error('Unsupported API route.')
    if (this.control) {
      const scope = this.identity, epoch = this.epoch
      if (!scope) throw Error('Device session unavailable. Refresh to reconnect.')
      if (/^\/api\/automation\/(arm|review|live)$/.test(path)) throw new ApiError(422, 'unsupported-route', 'Remote automation review, live decisions and arming are unsupported. Use the owning local service.')
      const v = object(await this.request('/control/request', { deviceId: scope.deviceId, authorityId: scope.authorityId, revision: scope.revision, path, body }, true, path))
      this.assertEpoch(epoch)
      if (v.deviceId !== scope.deviceId || v.authorityId !== scope.authorityId || !Number.isSafeInteger(v.revision) || (v.revision as number) < scope.revision || !Number.isInteger(v.status) || !('body' in v)) {
        this.disconnect('Response identity does not match the requested device session.'); throw Error(this.connectionError)
      }
      if (!this.identity || (v.revision as number) < this.identity.revision) throw new StaleSessionError()
      this.identity = { ...scope, revision: v.revision as number }
      if ((v.status as number) < 200 || (v.status as number) >= 300) throw this.apiError(v.status as number, v.body)
      return v.body
    }
    if (!this.token) throw new Error('Service session unavailable. Refresh to reconnect.')
    return this.request(path, body)
  }
  private apiError(status: number, value: unknown) {
    const e = object(object(value).error)
    return new ApiError(status, string(e.code), string(e.message))
  }
  private async request(path: string, body?: unknown, control = false, canonicalPath = path): Promise<unknown> {
    const epoch = this.epoch, token = this.token
    // Serialization precedes I/O: later edits cannot retarget or change a write.
    const serialized = body === undefined ? undefined : (control || canonicalPath.startsWith('/api/dsp/') || canonicalPath.startsWith('/api/automation/')) ? dspJson(body) : JSON.stringify(body)
    const controller = new AbortController(), timeout = setTimeout(() => controller.abort(), 12_000)
    this.requests.add(controller)
    try {
      const response = await this.fetcher(path, {
        method: body === undefined ? 'GET' : 'POST', credentials: 'same-origin', cache: 'no-store',
        redirect: 'error',
        headers: body === undefined ? {} : control ? { 'Content-Type': 'application/json', Authorization: `Bearer ${this.control!.credential}` } : { 'Content-Type': 'application/json', 'X-Mixyard-Token': token! },
        ...(serialized === undefined ? {} : { body: serialized }), signal: controller.signal,
      })
      const text = await response.text()
      if (epoch !== this.epoch) throw new StaleSessionError()
      if (text.length > 2_097_152) throw new Error('Service response exceeds the supported size.')
      let value: unknown
      try { value = control || canonicalPath.startsWith('/api/automation/') ? parseAutomationJson(text) : JSON.parse(text) } catch (error) { throw new Error(`Service returned unreadable JSON (HTTP ${response.status}): ${error instanceof Error && !(error instanceof SyntaxError) ? error.message : 'Invalid JSON'}. Refresh to reconnect.`) }
      if (control && path === '/control/request' && 'body' in object(value)) {
        if (object(value).status !== response.status) { this.disconnect('Control response status mismatch.'); throw Error(this.connectionError) }
      } else if (!response.ok) {
        const error = this.apiError(response.status, value)
        if ([401, 403].includes(response.status) || (control && response.status === 409)) this.disconnect(`${error.code}: ${error.message}`)
        throw error
      }
      return value
    } catch (error) {
      if (epoch !== this.epoch && !(error instanceof ApiError)) throw new StaleSessionError()
      if (error instanceof TypeError || (error instanceof Error && error.name === 'AbortError')) { this.disconnect(this.control ? 'Device service offline or timed out. Refresh to reconnect. Local edits are kept.' : 'Local service offline or timed out. Start mixyard-web, then Refresh. Local edits are kept.'); throw new Error(this.connectionError) }
      throw error
    } finally { clearTimeout(timeout); this.requests.delete(controller) }
  }
}

export type ApiClient = Pick<Api, keyof Api>
