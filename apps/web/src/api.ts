import { object, string } from './model.ts'

export class ApiError extends Error {
  status: number
  code: string
  constructor(status: number, code: string, message: string) { super(message); this.status = status; this.code = code }
}
export class Api {
  token: string | null = null
  private fetcher: typeof fetch
  constructor(fetcher: typeof fetch = (input, init) => fetch(input, init)) { this.fetcher = fetcher }
  async connect() {
    this.token = null
    const v = object(await this.request('/api/session'))
    if (v.apiVersion !== 1 || v.audioAvailable !== false) throw new Error('Unsupported API session. Reload after updating the service.')
    this.token = string(v.csrfToken)
  }
  async post(path: string, body: unknown = {}): Promise<unknown> {
    if (!this.token) throw new Error('Service session unavailable. Refresh to reconnect.')
    return this.request(path, body)
  }
  private async request(path: string, body?: unknown): Promise<unknown> {
    const controller = new AbortController(), timeout = setTimeout(() => controller.abort(), 12_000)
    try {
      const response = await this.fetcher(path, {
        method: body === undefined ? 'GET' : 'POST', credentials: 'same-origin', cache: 'no-store',
        headers: body === undefined ? {} : { 'Content-Type': 'application/json', 'X-Mixyard-Token': this.token! },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }), signal: controller.signal,
      })
      const text = await response.text()
      if (text.length > 2_097_152) throw new Error('Service response exceeds the supported size.')
      let value: unknown
      try { value = JSON.parse(text) } catch { throw new Error(`Service returned unreadable JSON (HTTP ${response.status}). Refresh to reconnect.`) }
      if (!response.ok) {
        const e = object(object(value).error)
        throw new ApiError(response.status, string(e.code), string(e.message))
      }
      return value
    } catch (error) {
      if (error instanceof TypeError || (error instanceof Error && error.name === 'AbortError')) throw new Error('Local service offline or timed out. Start mixyard-web, then Refresh. Local edits are kept.')
      throw error
    } finally { clearTimeout(timeout) }
  }
}
