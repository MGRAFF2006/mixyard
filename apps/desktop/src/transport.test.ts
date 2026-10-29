import assert from 'node:assert/strict'
import test from 'node:test'
import { desktopFetcher } from './transport.ts'
import type { Invoke } from './transport.ts'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { Api, StaleSessionError } from '../../web/src/api.ts'
import { ConsoleSession } from '../../web/src/session.ts'
import { readConfig, signature } from '../../web/src/model.ts'

// The API owns sibling-web reactive state. Independent locked installs may
// contain two Vue instances; use the API's runtime, as production Vite dedupes it.
const { watch } = createRequire(new URL('../../web/package.json', import.meta.url))('vue') as typeof import('vue')

test('native fetch preserves serialized precision and conflict status without retry', async () => {
  const calls: unknown[] = []
  const invoke: Invoke = async <T>(command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args }); return { status: 409, body: '{"error":{"code":"stale-revision"}}' } as T
  }
  const body = '{"nanoseconds":18446744073709551615}'
  const response = await desktopFetcher(invoke)('/api/dsp/delay', { method: 'POST', body, headers: { 'X-Mixyard-Token': 'a'.repeat(64) } })
  assert.equal(response.status, 409)
  assert.deepEqual(calls, [{ command: 'console_request', args: { request: { path: '/api/dsp/delay', method: 'POST', body, token: 'a'.repeat(64) } } }])
})

test('native fetch rejects remote origins and bearer credentials before native code', async () => {
  let calls = 0
  const invoke: Invoke = async () => { calls++; throw new Error('unexpected native operation') }
  const fetcher = desktopFetcher(invoke)
  await assert.rejects(fetcher('https://host/api/status'), /Remote control is unsupported/)
  await assert.rejects(fetcher('/control/request'), /Remote control is unsupported/)
  await assert.rejects(fetcher('/api/status', { headers: { Authorization: 'Bearer secret' } }), /does not forward authorization/)
  assert.equal(calls, 0)
})

test('aborted in-flight native write is not replayed or published as success', async () => {
  const controller = new AbortController()
  let calls = 0
  let complete!: (value: { status: number; body: string }) => void
  const pending = new Promise<{ status: number; body: string }>(resolve => { complete = resolve })
  const invoke: Invoke = async <T>() => { calls++; return await pending as T }
  const operation = desktopFetcher(invoke)('/api/profiles/edit', { method: 'POST', body: '{}', signal: controller.signal })
  controller.abort()
  complete({ status: 200, body: '{}' })
  await assert.rejects(operation, { name: 'AbortError' })
  assert.equal(calls, 1)
})

test('native string failures invalidate the shared session, retain drafts and fence older replies until explicit refresh', async () => {
  const config = readConfig(JSON.parse(readFileSync(new URL('../../web/src/example-config.json', import.meta.url), 'utf8')))
  const calls: string[] = []
  let failed = false
  let release!: (value: { status: number; body: string }) => void
  const held = new Promise<{ status: number; body: string }>(resolve => { release = resolve })
  const invoke: Invoke = async <T>(_command: string, args?: Record<string, unknown>) => {
    const path = (args!.request as { path: string }).path; calls.push(path)
    if (path === '/api/validate') return await held as T
    if (path === '/api/status' && failed) throw 'Local console unavailable: Connection refused (os error 111)'
    const body = path === '/api/session' ? { apiVersion: 1, csrfToken: 'owned-local-token', audioAvailable: false }
      : path === '/api/status' ? { connected: false, snapshot: null, error: null }
      : { format: 'mixyard-local-drafts', schemaVersion: 1, storeRevision: 0, configRevision: 1, profiles: [], draft: config, previousDraft: null }
    return { status: 200, body: JSON.stringify(body) } as T
  }
  const api = new Api(desktopFetcher(invoke)), session = new ConsoleSession(api)
  const stop = watch(() => api.epoch, () => session.invalidate(), { flush: 'sync' })
  try {
    await session.refresh(); assert.equal(session.canWrite, true)
    session.local!.revision++; session.validated = true
    const draft = signature(session.local), epoch = api.epoch
    const old = api.post('/api/validate', { config: session.local }).then(value => ({ value, error: null }), error => ({ value: null, error }))
    failed = true
    await assert.rejects(api.post('/api/status'))
    assert.equal(api.connected, false); assert.equal(api.epoch, epoch + 1)
    assert.equal(session.canWrite, false); assert.equal(session.needsRefresh, true)
    assert.equal(session.snapshot, null); assert.equal(session.validated, false)
    assert.equal(signature(session.local), draft)
    release({ status: 200, body: '{"valid":true}' }); assert.ok((await old).error instanceof StaleSessionError)
    assert.equal(calls.filter(path => path === '/api/validate').length, 1)
    assert.equal(calls.includes('/api/profiles/edit'), false)
    failed = false; await session.refresh()
    assert.equal(session.canWrite, true); assert.equal(signature(session.local), draft)
  } finally { stop(); release({ status: 200, body: '{"valid":true}' }) }
})
