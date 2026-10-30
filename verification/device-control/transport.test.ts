import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { once } from 'node:events'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { createServer, request } from 'node:https'
import type { Server } from 'node:https'
import type { Duplex } from 'node:stream'
import type { ServerResponse } from 'node:http'
import { join } from 'node:path'
import { test } from 'node:test'
import { pinnedFetch } from './pinned-fetch.ts'

test('pinned binding checks leaf identity and TLS version before sending HTTP, with fetch-style failures', async () => {
  const directory = mkdtempSync('/dev/shm/mixyard-control-pin-')
  const servers: Server[] = [], sockets = new Set<Duplex>()
  const file = (name: string) => join(directory, name)
  const openssl = (...args: string[]) => execFileSync('openssl', args, { stdio: ['ignore', 'ignore', 'pipe'] })
  try {
    // The supplied identity can also sign another valid server leaf. CA trust
    // alone must not turn that different leaf into the pinned device identity.
    openssl('req', '-x509', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:prime256v1', '-nodes', '-days', '1', '-subj', '/CN=Owned pin fixture', '-addext', 'subjectAltName=IP:127.0.0.1', '-addext', 'basicConstraints=critical,CA:TRUE', '-addext', 'keyUsage=critical,digitalSignature,keyCertSign,cRLSign', '-addext', 'extendedKeyUsage=serverAuth', '-keyout', file('identity.key'), '-out', file('identity.pem'))
    openssl('req', '-new', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:prime256v1', '-nodes', '-subj', '/CN=Owned other leaf', '-keyout', file('other.key'), '-out', file('other.csr'))
    writeFileSync(file('other.ext'), 'subjectAltName=IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n')
    openssl('x509', '-req', '-in', file('other.csr'), '-CA', file('identity.pem'), '-CAkey', file('identity.key'), '-set_serial', '2', '-days', '1', '-extfile', file('other.ext'), '-out', file('other.pem'))
    const certificate = readFileSync(file('identity.pem'), 'utf8')
    async function listen(cert: string, key: string, version: 'TLSv1.3' | 'TLSv1.2', handler?: (reply: ServerResponse) => void) {
      let received = 0
      const server = createServer({ cert: readFileSync(file(cert)), key: readFileSync(file(key)), minVersion: version, maxVersion: version }, (_request, response) => { received++; if (handler) handler(response); else response.end('{}') })
      servers.push(server)
      server.on('connection', socket => { sockets.add(socket); socket.on('close', () => sockets.delete(socket)) })
      const listening = once(server, 'listening'); server.listen(0, '127.0.0.1'); await listening
      const address = server.address(); assert.ok(address && typeof address !== 'string')
      return { url: `https://127.0.0.1:${address.port}`, count: () => received }
    }
    async function ordinaryTrust(url: string, version: 'TLSv1.3' | 'TLSv1.2') {
      return new Promise<number>((resolve, reject) => {
        const operation = request(url, { ca: certificate, rejectUnauthorized: true, minVersion: version, maxVersion: version }, reply => { reply.resume(); reply.on('end', () => resolve(reply.statusCode!)) })
        operation.on('error', reject); operation.end()
      })
    }
    const expected = await listen('identity.pem', 'identity.key', 'TLSv1.3')
    const changed = await listen('other.pem', 'other.key', 'TLSv1.3')
    const olderTls = await listen('identity.pem', 'identity.key', 'TLSv1.2')
    const init = { method: 'POST', body: '{}', headers: { Authorization: 'Bearer owned-test-sentinel' } }
    assert.equal((await pinnedFetch(expected.url, certificate)('/control/session', init)).status, 200)
    assert.equal(expected.count(), 1)
    assert.equal(await ordinaryTrust(changed.url, 'TLSv1.3'), 200, 'the alternate leaf must really pass normal PKI/name verification')
    await assert.rejects(pinnedFetch(changed.url, certificate)('/control/session', init), (error: unknown) => error instanceof TypeError && error.cause instanceof Error && /pin mismatch/.test(error.cause.message))
    assert.equal(changed.count(), 1, 'pin refusal happens before an HTTP request or credential is sent')
    assert.equal(await ordinaryTrust(olderTls.url, 'TLSv1.2'), 200)
    await assert.rejects(pinnedFetch(olderTls.url, certificate)('/control/session', init), TypeError)
    assert.equal(olderTls.count(), 1, 'TLS version refusal happens before HTTP')
    const entered = Promise.withResolvers<void>()
    let heldResponse: ServerResponse | undefined
    const blocked = await listen('identity.pem', 'identity.key', 'TLSv1.3', response => {
      heldResponse = response; entered.resolve()
    })
    const fetcher = pinnedFetch(blocked.url, certificate)
    const first = fetcher('/control/session', init)
    await entered.promise
    const cancel = new AbortController()
    const queued = fetcher('/control/session', { ...init, signal: cancel.signal })
    const rejected = assert.rejects(queued, (error: unknown) => error instanceof Error && error.name === 'AbortError')
    cancel.abort()
    let timer: ReturnType<typeof setTimeout> | undefined
    try {
      await Promise.race([
        rejected,
        new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error('Queued abort waited for the preceding HTTP response.')), 250) }),
      ])
      assert.equal(blocked.count(), 1, 'cancelled queued requests send no HTTP')
    } finally {
      clearTimeout(timer)
      heldResponse!.end('{}')
      await first
      await rejected
    }
    assert.equal(blocked.count(), 1, 'cancelled requests stay absent after the predecessor completes')
  } finally {
    for (const socket of sockets) socket.destroy()
    await Promise.all(servers.map(server => new Promise<void>(resolve => server.close(() => resolve()))))
    rmSync(directory, { recursive: true, force: true })
  }
})
