import { request } from 'node:https'

// Test transport for an explicitly provisioned, owned TLS service. Trust is
// supplied by the harness, never discovered from the endpoint being tested.
export function pinnedFetch(endpoint: string, certificate: string): typeof fetch {
  const base = new URL(endpoint)
  if (base.protocol !== 'https:' || base.username || base.password || base.pathname !== '/' || base.search || base.hash) {
    throw new Error('Expected an exact HTTPS service origin.')
  }
  return (input, init = {}) => new Promise<Response>((resolve, reject) => {
    if (typeof input !== 'string' || !['/control/pair', '/control/session', '/control/request', '/control/credentials', '/control/rotate', '/control/revoke'].includes(input)) {
      reject(new Error('Only explicit control endpoints are admitted.')); return
    }
    if (init.method !== 'POST' || typeof init.body !== 'string') {
      reject(new Error('Control requests require a JSON POST.')); return
    }
    const headers = new Headers(init.headers)
    if ([...headers.keys()].some(key => !['authorization', 'content-type'].includes(key))) {
      reject(new Error('Unexpected control request header.')); return
    }
    if (init.signal?.aborted) { reject(init.signal.reason); return }
    const target = new URL(input, base)
    const pending = request(target, { method: 'POST', headers: Object.fromEntries(headers), ca: certificate, rejectUnauthorized: true, timeout: 12_000 }, response => {
      let size = 0
      const chunks: Buffer[] = []
      response.on('data', (chunk: Buffer) => {
        size += chunk.length
        if (size > 8 * 1024 * 1024) { pending.destroy(new Error('Control response exceeds 8 MiB.')); return }
        chunks.push(chunk)
      })
      response.on('error', reject)
      response.on('end', () => {
        const status = response.statusCode ?? 0
        if (status >= 300 && status < 400) { reject(new Error('Control redirects are forbidden.')); return }
        resolve(new Response(Buffer.concat(chunks), { status, headers: { 'Content-Type': String(response.headers['content-type'] ?? '') } }))
      })
    })
    const abort = () => pending.destroy(init.signal?.reason instanceof Error ? init.signal.reason : new Error('Control request aborted.'))
    init.signal?.addEventListener('abort', abort, { once: true })
    pending.on('timeout', () => pending.destroy(new Error('Control request timed out.')))
    pending.on('error', reject)
    pending.on('close', () => init.signal?.removeEventListener('abort', abort))
    pending.end(init.body)
  })
}
