import { Agent, request } from 'node:https'
import { X509Certificate } from 'node:crypto'
import { checkServerIdentity } from 'node:tls'

// Test transport for an explicitly provisioned, owned TLS service. Trust is
// supplied by the harness, never discovered from the endpoint being tested.
export function pinnedFetch(endpoint: string, certificate: string): typeof fetch {
  const base = new URL(endpoint)
  if (base.protocol !== 'https:' || base.username || base.password || base.pathname !== '/' || base.search || base.hash) {
    throw new Error('Expected an exact HTTPS service origin.')
  }
  const pinned = new X509Certificate(certificate).raw
  // A private policy pool cannot borrow another caller's authenticated socket.
  // One exchange at a time also preserves the service's explicit busy boundary
  // for independent controllers, without racing this client's parallel reads.
  const agent = new Agent({ keepAlive: false, maxSockets: 1, maxCachedSessions: 0 })
  return (input, init = {}) => new Promise<Response>((resolve, reject) => {
    const networkFailure = (cause: unknown) => reject(cause instanceof Error && cause.name === 'AbortError' ? cause : new TypeError('Control transport failed.', { cause }))
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
    const pending = request(target, {
      agent, method: 'POST', headers: Object.fromEntries(headers), ca: certificate, rejectUnauthorized: true,
      minVersion: 'TLSv1.3', maxVersion: 'TLSv1.3', timeout: 12_000,
      checkServerIdentity: (host, peer) => checkServerIdentity(host, peer) ?? (peer.raw.equals(pinned) ? undefined : new Error('Control TLS certificate pin mismatch.')),
    }, response => {
      let size = 0
      const chunks: Buffer[] = []
      response.on('data', (chunk: Buffer) => {
        size += chunk.length
        if (size > 8 * 1024 * 1024) { pending.destroy(new Error('Control response exceeds 8 MiB.')); return }
        chunks.push(chunk)
      })
      response.on('error', networkFailure)
      response.on('end', () => {
        const status = response.statusCode ?? 0
        if (status >= 300 && status < 400) { networkFailure(new Error('Control redirects are forbidden.')); return }
        try { resolve(new Response(Buffer.concat(chunks), { status, headers: { 'Content-Type': String(response.headers['content-type'] ?? '') } })) }
        catch (error) { networkFailure(error) }
      })
    })
    const abort = () => {
      const reason = init.signal?.reason ?? new DOMException('Aborted', 'AbortError')
      // A queued Agent request may not emit error/close until its predecessor
      // finishes. Cancellation settles independently of that socket admission.
      clearTimeout(deadline)
      init.signal?.removeEventListener('abort', abort)
      pending.destroy(reason instanceof Error ? reason : new Error('Control request aborted.'))
      reject(reason)
    }
    const deadline = setTimeout(() => pending.destroy(new Error('Control request total deadline exceeded.')), 12_000)
    init.signal?.addEventListener('abort', abort, { once: true })
    pending.on('timeout', () => pending.destroy(new Error('Control request timed out.')))
    pending.on('error', networkFailure)
    pending.on('close', () => { clearTimeout(deadline); init.signal?.removeEventListener('abort', abort) })
    pending.end(init.body)
  })
}
