import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { once } from 'node:events'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { Api, ApiError, StaleSessionError } from './api.ts'
import { ConsoleSession } from './session.ts'
import { copy, readConfig } from './model.ts'
import type { Snapshot } from './model.ts'
import { ActivationSession } from './activation-session.ts'
import { SituationSession } from './situation-session.ts'
import { reactive, watch } from 'vue'

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>(done => { resolve = done })
  return { promise, resolve }
}
const config = () => readConfig(JSON.parse(readFileSync(new URL('./example-config.json', import.meta.url), 'utf8')))
async function service(deviceId: string) {
  let authorityId = `${deviceId}-boot-1`, revoked = false, wrongHost = false
  let store: Snapshot = { format: 'mixyard-local-drafts', schemaVersion: 1, storeRevision: 0, configRevision: 1, profiles: [], draft: config(), previousDraft: null }
  let hold: { arrived: ReturnType<typeof deferred<void>>; release: ReturnType<typeof deferred<void>> } | null = null
  const calls: { path: string; revision: number; deviceId: string; authorityId: string; body: Record<string, unknown> }[] = []
  const server = createServer(async (req, res) => {
    const reply = (status: number, body: unknown) => { res.writeHead(status, { 'Content-Type': 'application/json' }); res.end(JSON.stringify(body)) }
    const error = (status: number, code: string) => reply(status, { error: { code, message: code } })
    if (req.headers.authorization !== `Bearer ${deviceId}-credential` || revoked) return error(401, 'unauthorized')
    assert.equal(req.headers['x-mixyard-token'], undefined)
    let text = ''
    for await (const chunk of req) text += String(chunk)
    const request = JSON.parse(text)
    if (req.url === '/control/session') return reply(200, { deviceId, authorityId, revision: store.storeRevision, permissions: ['view', 'control', 'settings'], backendAvailable: false, audioApplied: false })
    assert.equal(req.url, '/control/request')
    calls.push(copy(request))
    if (request.deviceId !== deviceId || request.authorityId !== authorityId) return error(409, 'session-changed')
    if (request.revision !== store.storeRevision) return error(409, 'store-conflict')
    if (hold && request.path === '/api/profiles/edit') {
      const pending = hold; hold = null; pending.arrived.resolve(); await pending.release.promise
      if (revoked) return error(401, 'unauthorized')
      if (request.authorityId !== authorityId) return error(409, 'session-changed')
      if (request.revision !== store.storeRevision) return error(409, 'store-conflict')
    }
    let body: unknown, status = 200
    if (request.path === '/api/profiles/read') body = copy(store)
    else if (request.path === '/api/status') body = { connected: false, snapshot: null, error: null }
    else if (request.path === '/api/validate') body = { valid: true }
    else if (request.path === '/api/profiles/edit') {
      if (request.body.expectedRevision !== store.storeRevision) { status = 409; body = { error: { code: 'store-conflict', message: 'Fresh read required' } } }
      else {
        store = { ...store, storeRevision: store.storeRevision + 1, configRevision: store.configRevision + 1, draft: copy(request.body.edit.config) }
        store.draft!.revision = store.configRevision
        body = copy(store)
      }
    } else { status = 422; body = { error: { code: 'unsupported-route', message: 'Not a fixture route' } } }
    reply(status, { deviceId: wrongHost ? 'wrong-host' : deviceId, authorityId, revision: store.storeRevision, status, body })
  })
  server.listen(0, '127.0.0.1'); await once(server, 'listening')
  const address = server.address(); assert.ok(address && typeof address !== 'string')
  const url = `http://127.0.0.1:${address.port}`
  // Deliberately ignore cancellation to prove rejection even if a transport
  // cannot cancel a dispatched operation. This loopback fixture is not TLS proof.
  const api = () => Api.forControl((input, init) => fetch(new URL(String(input), url), { ...init, signal: undefined }), { deviceId, credential: `${deviceId}-credential` })
  return {
    api, calls, get store() { return store },
    hold() { hold = { arrived: deferred<void>(), release: deferred<void>() }; return hold },
    revoke() { revoked = true }, restart() { authorityId = `${deviceId}-boot-2` }, wrongHost() { wrongHost = true },
    async close() { await new Promise<void>(resolve => server.close(() => resolve())); server.closeAllConnections() },
  }
}

test('loopback A/B: in-flight writes stay on original host; switch discards completion and never replays', async () => {
  const a = await service('A'), b = await service('B')
  try {
    const sa = new ConsoleSession(a.api()), sb = new ConsoleSession(b.api())
    await sa.refresh(); sa.local!.sends[0]!.gain = 0.21
    const held = a.hold(), pending = sa.edit({ kind: 'draft', config: copy(sa.local!) })
    await held.arrived.promise
    sa.api.disconnect(); sa.invalidate()
    await sb.refresh(); sb.local!.sends[0]!.gain = 0.73
    await sb.edit({ kind: 'draft', config: copy(sb.local!) })
    held.release.resolve(); await pending
    assert.equal(a.store.draft!.sends[0]!.gain, 0.21)
    assert.equal(b.store.draft!.sends[0]!.gain, 0.73)
    assert.equal(sa.snapshot, null)
    assert.equal(sa.needsRefresh, true)
    assert.equal(sa.local!.sends[0]!.gain, 0.21)
    await sa.refresh()
    assert.equal(sa.api.identity?.deviceId, 'A')
    assert.equal(a.calls.filter(c => c.path === '/api/profiles/edit').length, 1)
    assert.equal(b.calls.filter(c => c.path === '/api/profiles/edit').length, 1)
    assert.ok(a.calls.every(c => c.deviceId === 'A' && c.authorityId === 'A-boot-1'))
    assert.ok(b.calls.every(c => c.deviceId === 'B'))
  } finally { await Promise.all([a.close(), b.close()]) }
})

test('loopback revocation, restart and competing controllers invalidate authority without overwriting local drafts', async () => {
  const s = await service('B')
  try {
    const one = new ConsoleSession(s.api()), two = new ConsoleSession(s.api())
    await one.refresh(); await two.refresh()
    one.local!.sends[0]!.gain = 0.12; two.local!.sends[0]!.gain = 0.67
    await one.edit({ kind: 'draft', config: copy(one.local!) })
    await two.edit({ kind: 'draft', config: copy(two.local!) })
    assert.equal(two.api.connected, false)
    assert.equal(two.needsRefresh, true)
    assert.equal(two.local!.sends[0]!.gain, 0.67)
    assert.equal(s.store.draft!.sends[0]!.gain, 0.12)
    await two.refresh(); assert.equal(two.local!.sends[0]!.gain, 0.67)
    s.restart()
    await two.validate(); assert.equal(two.api.connected, false)
    await two.refresh(); assert.equal(two.api.identity?.authorityId, 'B-boot-2')
    const held = s.hold(), pending = two.edit({ kind: 'draft', config: copy(two.local!) })
    await held.arrived.promise; s.revoke(); held.release.resolve(); await pending
    assert.equal(two.api.connected, false)
    assert.equal(s.store.draft!.sends[0]!.gain, 0.12)
    assert.equal(two.local!.sends[0]!.gain, 0.67)
    assert.equal(two.canWrite, false)
    assert.equal(s.calls.filter(c => c.path === '/api/profiles/edit').length, 3)
  } finally { await s.close() }
})

test('wrong-host completions fail closed; remote review/live/arm never leave the client', async () => {
  const s = await service('A')
  try {
    const api = s.api(); await api.connect()
    for (const path of ['arm', 'review', 'live']) await assert.rejects(api.post(`/api/automation/${path}`), e => e instanceof ApiError && e.code === 'unsupported-route')
    assert.equal(s.calls.length, 0)
    s.wrongHost()
    await assert.rejects(api.post('/api/profiles/read'), /identity/)
    assert.equal(api.connected, false)
  } finally { await s.close() }
})

test('local reconnect rejects delayed old CSRF response and immutable request serializes before I/O', async () => {
  const held = deferred<Response>(), arrived = deferred<void>()
  let token = 'old', submitted: unknown
  const api = new Api(async (path, init) => {
    if (path === '/api/session') return Response.json({ apiVersion: 1, csrfToken: token, audioAvailable: false })
    submitted = JSON.parse(String(init?.body)); arrived.resolve(); return held.promise
  })
  await api.connect()
  const body = { expectedRevision: 1, edit: { kind: 'rename', name: 'Original' } }
  const result = assert.rejects(api.post('/api/profiles/edit', body), StaleSessionError)
  await arrived.promise; body.edit.name = 'Later draft'; token = 'new'; await api.connect()
  held.resolve(Response.json({ storeRevision: 2 })); await result
  assert.deepEqual(submitted, { expectedRevision: 1, edit: { kind: 'rename', name: 'Original' } })
  assert.equal(api.token, 'new')
})

test('disconnect drops collected context and arm review while retaining host-local builder/settings edits', async () => {
  const pending = deferred<unknown>(), api = { post: async () => pending.promise }
  const situations = new SituationSession(api), activation = new ActivationSession(api)
  situations.edit(r => { r.graph.name = 'A draft' })
  activation.edit(s => { s.observer.hostLabel = 'A host' })
  const collect = situations.collect()
  situations.disconnect(); activation.disconnect()
  pending.resolve({}); await collect
  assert.equal(situations.live, null)
  assert.equal(situations.liveError, '')
  assert.equal(situations.request.graph.name, 'A draft')
  assert.equal(activation.draft.observer.hostLabel, 'A host')
  assert.equal(activation.reviewCurrent, false)
  assert.equal(activation.connected, false)
  assert.equal(activation.saved, null)
  assert.equal(activation.needsRead, true)
})

test('control boundary validates integer identity metadata/capabilities and preserves canonical non-2xx errors', async () => {
  const good = { deviceId: 'A', authorityId: 'boot', revision: 7, permissions: ['settings'], backendAvailable: false, audioApplied: false }
  for (const bad of [
    { ...good, revision: 1.5 }, { ...good, revision: 2 ** 53 }, { ...good, authorityId: '' },
    { ...good, permissions: ['view', 'admin'] }, { ...good, permissions: ['view', 'view'] },
    { ...good, backendAvailable: true }, { ...good, audioApplied: true }, { ...good, deviceId: 'B' },
  ]) {
    const api = Api.forControl(async () => Response.json(bad), { deviceId: 'A', credential: 'private' })
    await assert.rejects(api.connect())
    assert.equal(api.connected, false)
  }
  let calls = 0
  const api = Api.forControl(async path => {
    calls++
    if (path === '/control/session') return Response.json(good)
    return Response.json({ deviceId: 'A', authorityId: 'boot', revision: 7, status: 422, body: { error: { code: 'invalid-policy', message: 'Unsupported exact ordered channel route' } } }, { status: 422 })
  }, { deviceId: 'A', credential: 'private' })
  await api.connect()
  await assert.rejects(api.post('/api/profiles/edit', {}), e => e instanceof ApiError && e.status === 422 && e.code === 'invalid-policy' && e.message === 'Unsupported exact ordered channel route')
  assert.equal(calls, 2)
  assert.equal(api.identity?.permissions.includes('control'), false)
})

test('an obsolete operation cannot release a new session busy guard', async () => {
  const old = deferred<unknown>(), fresh = deferred<unknown>()
  let n = 0
  const session = new SituationSession({ post: async () => ++n === 1 ? old.promise : fresh.promise })
  const first = session.validate()
  session.disconnect()
  const second = session.validate()
  old.resolve({ valid: true }); await first
  assert.equal(session.busy, true)
  assert.equal(session.validated, false)
  fresh.resolve({ valid: true }); await second
  assert.equal(session.busy, false)
  assert.equal(session.validated, true)
})

test('independent settings grant remains usable without view; permission denial is not credential revocation', async () => {
  const paths: string[] = []
  const api = Api.forControl(async (path, init) => {
    const wirePath = String(path), request = JSON.parse(String(init?.body)); paths.push(wirePath)
    if (wirePath === '/control/session') return Response.json({ deviceId: 'A', authorityId: 'boot', revision: 0, permissions: ['settings'], backendAvailable: false, audioApplied: false })
    if (request.path === '/api/automation/settings/read') return Response.json({ deviceId: 'A', authorityId: 'boot', revision: 0, status: 200, body: { settings: 'settings-only result' } })
    return Response.json({ error: { code: 'permission-denied', message: 'View was not granted' } }, { status: 403 })
  }, { deviceId: 'A', credential: 'private' })
  const console = new ConsoleSession(api)
  await console.refresh()
  assert.equal(api.connected, true)
  assert.equal(console.canWrite, false)
  assert.match(console.error, /View permission was not granted/)
  assert.deepEqual(paths, ['/control/session'])
  await assert.rejects(api.post('/api/status'), e => e instanceof ApiError && e.code === 'permission-denied')
  assert.equal(api.connected, true)
  assert.deepEqual(await api.post('/api/automation/settings/read'), { settings: 'settings-only result' })
})

test('scoped canonical conflicts revoke shared authority immediately, while permission and validation errors preserve grants', async () => {
  for (const [status, code, revokes] of [[409, 'store-conflict', true], [401, 'unauthorized', true], [403, 'invalid-origin', true], [403, 'permission-denied', false], [400, 'invalid-automation', false]] as const) {
    let requests = 0
    const api = Api.forControl(async (path) => {
      if (path === '/control/session') return Response.json({ deviceId: 'A', authorityId: 'boot', revision: 0, permissions: ['settings'], backendAvailable: false, audioApplied: false })
      requests++
      return Response.json({ deviceId: 'A', authorityId: 'boot', revision: 0, status, body: { error: { code, message: 'Original canonical reason' } } }, { status })
    }, { deviceId: 'A', credential: 'private' })
    await api.connect()
    const epoch = api.epoch
    await assert.rejects(api.post('/api/automation/settings/write', { expectedSettingsRevision: 0 }), e => e instanceof ApiError && e.status === status && e.code === code)
    assert.equal(api.connected, !revokes, code)
    assert.equal(api.epoch, epoch + (revokes ? 1 : 0), code)
    if (revokes) {
      assert.equal(api.identity, null)
      assert.match(api.connectionError, new RegExp(code))
      await assert.rejects(api.post('/api/automation/settings/read'), /Refresh to reconnect/)
    } else assert.deepEqual(api.identity?.permissions, ['settings'])
    assert.equal(requests, 1, 'no replay or follow-up request under a revoked generation')
  }
})

test('a delayed same-epoch canonical conflict still revokes after a newer successful profile reply', async () => {
  const held = deferred<Response>()
  let requests = 0
  const api = Api.forControl(async (path, init) => {
    if (path === '/control/session') return Response.json({ deviceId: 'A', authorityId: 'boot', revision: 0, permissions: ['control', 'settings'], backendAvailable: false, audioApplied: false })
    requests++
    if (JSON.parse(String(init?.body)).path === '/api/automation/settings/write') return held.promise
    return Response.json({ deviceId: 'A', authorityId: 'boot', revision: 1, status: 200, body: {} })
  }, { deviceId: 'A', credential: 'private' })
  await api.connect()
  const epoch = api.epoch
  const pending = api.post('/api/automation/settings/write', { expectedSettingsRevision: 0 })
  const rejected = assert.rejects(pending, e => e instanceof ApiError && e.code === 'store-conflict')
  await api.post('/api/profiles/edit', {})
  assert.equal(api.identity!.revision, 1)
  held.resolve(Response.json({ deviceId: 'A', authorityId: 'boot', revision: 0, status: 409, body: { error: { code: 'store-conflict', message: 'Reread settings before writing' } } }, { status: 409 }))
  await rejected
  assert.equal(api.connected, false)
  assert.equal(api.identity, null)
  assert.equal(api.epoch, epoch + 1)
  await assert.rejects(api.post('/api/automation/settings/read'), /Refresh to reconnect/)
  assert.equal(requests, 2, 'no replay or follow-up request under the revoked generation')
})

test('consumer continuations cannot restore validation, inspection or a snapshot after concurrent authority loss', async () => {
  const snapshot: Snapshot = { format: 'mixyard-local-drafts', schemaVersion: 1, storeRevision: 0, configRevision: 1, profiles: [], draft: config(), previousDraft: null }
  const inventory = JSON.parse(readFileSync(new URL('./example-inventory.json', import.meta.url), 'utf8'))
  const api = reactive(new Api(async path => {
    if (path === '/api/session') return Response.json({ apiVersion: 1, csrfToken: 'local', audioAvailable: false })
    if (path === '/api/profiles/read' || path === '/api/profiles/edit') return Response.json(snapshot)
    if (path === '/api/status') return Response.json({ connected: false, snapshot: null, error: null })
    if (path === '/api/validate') return Response.json({ valid: true })
    if (path === '/api/inspect') return Response.json({ schemaVersion: 1, mode: 'routing-inspection', configRevision: config().revision, inventoryRevision: inventory.revision, inventoryEvidence: inventory.evidence, nativeState: 'unknown', liveLevels: null, planAvailable: false, preflightError: 'Fixture boundary', plan: null, findings: [], sources: [], mixes: [], sends: [], outputs: [], capacity: [] })
    throw Error(`Unexpected fixture route ${String(path)}`)
  }))
  const session = reactive(new ConsoleSession(api))
  const stop = watch(() => api.epoch, () => session.invalidate(), { flush: 'sync' })
  const post = api.post.bind(api)
  let revokeAfter = ''
  api.post = (path, body) => {
    const result = post(path, body)
    // Observe a real Api completion before its awaiting consumer resumes. This
    // models a concurrent revocation in the promise-continuation gap.
    void result.then(() => { if (path === revokeAfter) api.disconnect('Controller revoked') }, () => {})
    return result
  }
  try {
    for (const path of ['/api/validate', '/api/inspect', '/api/profiles/edit']) {
      revokeAfter = ''; await session.refresh(); session.inventory = inventory
      revokeAfter = path
      if (path === '/api/validate') await session.validate()
      else if (path === '/api/inspect') await session.inspect()
      else await session.edit({ kind: 'draft', config: config() })
      assert.equal(api.connected, false)
      assert.equal(session.validated, false)
      assert.equal(session.inspection, null)
      assert.equal(session.snapshot, null)
      assert.equal(session.notice, '')
    }
  } finally { stop() }
})

test('a rejected old save cannot pause writes after a fresh successful reconnect', async () => {
  const s = await service('A')
  try {
    const session = new ConsoleSession(s.api())
    await session.refresh()
    const held = s.hold(), old = session.edit({ kind: 'draft', config: copy(session.local!) })
    await held.arrived.promise
    session.api.disconnect(); session.invalidate(); await session.refresh()
    assert.equal(session.needsRefresh, false)
    assert.equal(session.canWrite, true)
    const fresh = session.snapshot
    held.release.resolve(); await old
    assert.equal(session.snapshot, fresh)
    assert.equal(session.needsRefresh, false)
    assert.equal(session.canWrite, true)
    assert.equal(session.error, '')
  } finally { await s.close() }
})

test('a host-held Api lifecycle change notifies consumers through the original reference', async () => {
  const host = new Api(async () => Response.json({ apiVersion: 1, csrfToken: 'local', audioAvailable: false }))
  const observed = reactive(host), epochs: number[] = []
  const stop = watch(() => observed.epoch, value => epochs.push(value), { flush: 'sync' })
  try {
    await observed.connect()
    const before = epochs.length
    host.disconnect('Host revoked this session')
    assert.equal(epochs.length, before + 1)
    assert.equal(observed.connected, false)
    assert.equal(observed.connectionError, 'Host revoked this session')
  } finally { stop() }
})
