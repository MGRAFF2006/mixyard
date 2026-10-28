import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { Api, ApiError } from './api.ts'
import { ConsoleSession } from './session.ts'
import { copy, parseImport, readConfig, readInspection, readInventory, readSnapshot, setLevel, signature } from './model.ts'
import type { Snapshot } from './model.ts'

const config = () => readConfig(JSON.parse(readFileSync(new URL('./example-config.json', import.meta.url), 'utf8')))
const inventory = () => readInventory(JSON.parse(readFileSync(new URL('./example-inventory.json', import.meta.url), 'utf8')))
const snapshot = (): Snapshot => ({ format: 'mixyard-local-drafts', schemaVersion: 1, storeRevision: 4, configRevision: 1, profiles: [], draft: config(), previousDraft: null })
function harness() {
  let store = snapshot(), conflict = false, failure = false, malformed = false
  const calls: { path: string; body: Record<string, unknown> | null }[] = []
  const api = new Api(async (input, init) => {
    const path = String(input), body = init?.body ? JSON.parse(String(init.body)) as Record<string, unknown> : null
    calls.push({ path, body })
    if (path !== '/api/session') {
      assert.equal(init?.method, 'POST')
      assert.equal(new Headers(init?.headers).get('X-Mixyard-Token'), 'process-token')
      assert.equal(new Headers(init?.headers).get('Content-Type'), 'application/json')
    }
    let result: unknown
    if (path === '/api/session') result = { apiVersion: 1, csrfToken: 'process-token', audioAvailable: false }
    else if (path === '/api/status') result = { connected: false, snapshot: null, error: 'Runtime not started' }
    else if (path === '/api/profiles/read') result = copy(store)
    else if (path === '/api/profiles/edit') {
      if (conflict) return Response.json({ error: { code: 'store-conflict', message: 'Changed elsewhere' } }, { status: 409 })
      if (failure) throw new TypeError('Disconnected')
      if (malformed) return Response.json({ schemaVersion: 1 })
      assert.equal(body?.expectedRevision, store.storeRevision)
      const edit = body?.edit as { kind: string; config: unknown; name: string }
      store.storeRevision++
      if (edit.kind === 'draft') { store.previousDraft = store.draft; store.draft = copy(readConfig(edit.config)); store.draft.revision = ++store.configRevision }
      else if (edit.kind === 'save') store.profiles.push({ id: 'profile-5', name: edit.name, config: copy(readConfig(edit.config)) })
      result = copy(store)
    } else if (path === '/api/validate') result = { valid: true }
    else throw new Error(`Unexpected request ${path}`)
    return Response.json(result)
  })
  return { session: new ConsoleSession(api), calls, get store() { return store }, conflict() { conflict = true; store.storeRevision = 9 }, resolve() { conflict = false }, offline() { failure = true }, malformed() { malformed = true } }
}

test('source sends, master and outputs edit independently; unrelated schema fields survive round trip', () => {
  const c = config(), original = copy(c)
  const call = c.sends.find(s => s.source === 'mic' && s.mix === 'call')!
  const record = c.sends.find(s => s.source === 'mic' && s.mix === 'record')!
  setLevel(call, { gain: 0.37, mute: true })
  assert.equal(record.gain, 0.5)
  assert.deepEqual(c.mixes, original.mixes)
  assert.deepEqual(c.outputs, original.outputs)
  const output = c.outputs[0]!
  setLevel(output, { gain: 0.23, mute: true })
  assert.deepEqual(c.outputs.slice(1), original.outputs.slice(1))
  assert.deepEqual(c.mediaControls, original.mediaControls)
  assert.deepEqual(c.sources, original.sources)
  assert.deepEqual(readConfig(parseImport(signature(c))), c)
  const withUneditedFields = { ...c, futureField: { keep: true } }
  assert.deepEqual(readConfig(withUneditedFields).futureField, { keep: true })
  assert.throws(() => setLevel(call, { gain: NaN }), /Gain/)
  assert.equal(call.gain, 0.37)
})

test('imports reject wrong schema, duplicate cells and unsafe revisions without coercion', () => {
  assert.throws(() => readConfig({ ...config(), revision: '2' }), /integer/)
  assert.throws(() => readConfig({ ...config(), schemaVersion: 1 }), /schema-2/)
  const c = config(); c.sends.push(copy(c.sends[0]!))
  assert.throws(() => readConfig(c), /Duplicate/)
  assert.throws(() => parseImport('{"revision":9007199254740993}'), /exact/)
  assert.throws(() => parseImport(' '.repeat(131073)), /128 KiB/)
  assert.throws(() => readInventory({ ...inventory(), evidence: 'live' }), /simulated/)
  assert.throws(() => readSnapshot({ ...snapshot(), draft: {} }), /schema-2/)
})

test('fresh reads establish revision, stored draft save reopens exactly with the server revision', async () => {
  const h = harness(), s = h.session
  await s.refresh()
  assert.equal(s.canWrite, true)
  assert.equal(s.dirty, false)
  s.local!.sends[0]!.gain = 0.37; s.changed()
  await s.edit({ kind: 'draft', config: copy(s.local!) })
  assert.equal(s.dirty, false)
  assert.equal(s.local!.revision, 2)
  assert.equal(h.calls.find(c => c.path === '/api/profiles/edit')?.body?.expectedRevision, 4)
  const reopened = new ConsoleSession(s.api)
  await reopened.refresh()
  assert.equal(reopened.local!.sends[0]!.gain, 0.37)
  assert.equal(reopened.local!.sends[1]!.gain, 0.5)
})

test('profile save preserves unsaved draft and does not claim it persisted', async () => {
  const h = harness(), s = h.session
  await s.refresh(); s.local!.sends[0]!.gain = 0.37; s.changed()
  const baseline = s.baseline
  await s.edit({ kind: 'save', id: null, name: 'Studio', config: copy(s.local!) })
  assert.equal(s.dirty, true)
  assert.equal(s.baseline, baseline)
  assert.equal(s.snapshot!.draft!.sends[0]!.gain, 1)
  assert.equal(s.snapshot!.profiles[0]!.config.sends[0]!.gain, 0.37)
  assert.match(s.notice, /Use Save draft/)
})

test('409 keeps local edits, pauses writes, and refresh never retries or overwrites them', async () => {
  const h = harness(), s = h.session
  await s.refresh(); s.local!.sends[0]!.gain = 0.37; s.changed()
  const local = signature(s.local)
  h.conflict()
  await s.edit({ kind: 'draft', config: copy(s.local!) })
  assert.match(s.error, /Store conflict/)
  assert.equal(signature(s.local), local)
  assert.equal(s.canWrite, false)
  await s.edit({ kind: 'draft', config: copy(s.local!) })
  assert.equal(h.calls.filter(c => c.path === '/api/profiles/edit').length, 1)
  h.resolve(); await s.refresh()
  assert.equal(s.snapshot!.storeRevision, 9)
  assert.equal(signature(s.local), local)
  assert.equal(s.dirty, true)
  assert.equal(h.calls.filter(c => c.path === '/api/profiles/edit').length, 1)
  s.reload()
  assert.equal(s.local!.sends[0]!.gain, 1)
  assert.equal(s.dirty, false)
})

test('offline or malformed write results preserve edits and require a read before another write', async () => {
  for (const fail of ['offline', 'malformed'] as const) {
    const h = harness(), s = h.session
    await s.refresh(); s.local!.sends[0]!.gain = 0.37
    const local = signature(s.local)
    h[fail]()
    await s.edit({ kind: 'draft', config: copy(s.local!) })
    assert.equal(signature(s.local), local)
    assert.equal(s.canWrite, false)
    assert.ok(s.error)
  }
})

test('inspection results clear on edits and malformed audio claims are rejected', () => {
  const s = new ConsoleSession()
  s.validated = true
  s.inspection = {} as never
  s.changed()
  assert.equal(s.inspection, null)
  assert.equal(s.validated, false)
  assert.throws(() => readInspection({ schemaVersion: 1, mode: 'routing-inspection', liveLevels: 0.8 }), /Unsupported/)
})

test('API rejects unsupported sessions and preserves structured failures; does not retry writes', async () => {
  const bad = new Api(async () => Response.json({ apiVersion: 2, audioAvailable: false, csrfToken: 'token' }))
  await assert.rejects(bad.connect(), /Unsupported API session/)
  assert.equal(bad.token, null)
  let requests = 0
  const api = new Api(async () => { requests++; return Response.json({ error: { code: 'invalid-policy', message: 'Headroom exceeded' } }, { status: 422 }) })
  api.token = 'token'
  await assert.rejects(api.post('/api/validate', {}), error => error instanceof ApiError && error.code === 'invalid-policy' && error.message === 'Headroom exceeded')
  assert.equal(requests, 1)
  const unreadable = new Api(async () => new Response('<html>service missing</html>', { status: 502 }))
  unreadable.token = 'token'
  await assert.rejects(unreadable.post('/api/profiles/read'), /unreadable JSON/)
})
