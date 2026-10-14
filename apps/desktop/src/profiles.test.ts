import assert from 'node:assert/strict'
import { test } from 'node:test'
import { ProfileSession, previewRequest, readProfiles, readProfilePreview, MAX_INVENTORY_BYTES } from './profiles.ts'
const config = { schemaVersion: 2, revision: 1, sources: [], sends: [], mixes: [], outputs: [], mediaControls: [] }
const safety = { schemaVersion: 1, mode: 'local-profile-preview', provenance: 'current-user-local-store', freshness: 'read-snapshot', storeRevision: 7, audioApplied: false, confirmedWorkingSetup: null, storedConsentIsFreshNativeConsent: false }
const list = () => ({ ...safety, command: 'list', storeState: 'present', profiles: [{ id: 'a', name: '../夜 ♥', config }, { id: 'b', name: '../夜 ♥', config }], draft: config })
const request = () => previewRequest(7, 'a', '{"schemaVersion":2,"revision":12}')
const response = () => ({ ...safety, command: 'preview', profileId: 'a', inventoryRevision: 12, draftRevision: 1, preview: {
  changes: { sources: [], sends: [], mixes: [], masters: [], outputs: [], mediaControls: [] },
  affectedSensitivePathsBefore: [], affectedSensitivePathsAfter: [], sensitivePaths: [], unavailableIdentities: { sources: [], outputs: [] },
  plan: { schemaVersion: 2, mode: 'dry-run', configRevision: 2, inventoryRevision: 12, inventoryEvidence: 'simulated', platform: 'linux', sources: [], mixes: [], outputs: [], diagnostics: [] },
} })
const readList = () => readProfiles(JSON.stringify(list()))
const readPreview = () => readProfilePreview(JSON.stringify(response()), request())

test('local profiles preserve duplicate/path-looking Unicode names and select by stable ID', () => {
  const snapshot = readList()
  assert.equal(snapshot.profiles.length, 2)
  assert.equal(snapshot.profiles[0]!.name, snapshot.profiles[1]!.name)
  const session = new ProfileSession(); session.snapshot = snapshot; session.select('b')
  assert.equal(session.selectedId, 'b'); session.select('../夜 ♥'); assert.equal(session.selectedId, '')
  assert.equal(readProfiles(JSON.stringify({ ...list(), storeRevision: 0, storeState: 'missing', profiles: [], draft: null })).storeState, 'missing')
})
test('bounds, exact revisions and evidence labels fail closed', () => {
  for (const inventory of ['{', '[]', '{"schemaVersion":1,"revision":0}', '{"schemaVersion":2,"revision":9007199254740992}', 'é'.repeat(MAX_INVENTORY_BYTES / 2 + 1)]) assert.throws(() => previewRequest(7, 'a', inventory))
  for (const mutation of [{ audioApplied: true }, { confirmedWorkingSetup: {} }, { freshness: 'live' }, { storeRevision: 2 ** 53 }, { profiles: [list().profiles[0], list().profiles[0]] }]) assert.throws(() => readProfiles(JSON.stringify({ ...list(), ...mutation })))
  for (const mutation of [{ storeRevision: 8 }, { profileId: 'b' }, { inventoryRevision: 13 }, { draftRevision: 2 ** 53 }, { audioApplied: true }]) assert.throws(() => readProfilePreview(JSON.stringify({ ...response(), ...mutation }), request()))
  const v = response(); v.preview.plan.configRevision = 2 ** 53
  assert.throws(() => readProfilePreview(JSON.stringify(v), request()))
  assert.equal(readPreview().inventoryRevision, 12)
})
test('refresh, selection and inventory changes clear selection/results and discard late native replies', async () => {
  const s = new ProfileSession(); await s.refresh(async () => readList()); s.select('a')
  await s.preview(request().inventoryJson, async () => readPreview()); assert.ok(s.result)
  s.inputChanged(); assert.equal(s.selectedId, ''); assert.equal(s.result, null)
  s.select('a')
  let resolve!: (v: ReturnType<typeof readPreview>) => void
  const pending = s.preview(request().inventoryJson, () => new Promise(done => { resolve = done }))
  s.select('b'); resolve(readPreview()); await pending
  assert.equal(s.result, null); assert.equal(s.selectedId, 'b')
  await s.refresh(async () => readList()); assert.equal(s.selectedId, ''); assert.equal(s.result, null)
  s.select('a')
  const disposing = s.preview(request().inventoryJson, () => new Promise(done => { resolve = done }))
  s.dispose(); resolve(readPreview()); await disposing; assert.equal(s.result, null)
})
test('store conflict, corrupt/native failures and overlapping requests invalidate all old context', async () => {
  const s = new ProfileSession(); await s.refresh(async () => readList()); s.select('a')
  await s.preview(request().inventoryJson, async () => { throw new Error('store-conflict') })
  assert.equal(s.snapshot, null); assert.equal(s.selectedId, ''); assert.equal(s.error, 'store-conflict')
  let resolve!: (v: ReturnType<typeof readList>) => void
  const pending = s.refresh(() => new Promise(done => { resolve = done }))
  await s.refresh(async () => assert.fail('No overlapping invocation'))
  assert.match(s.error, /already running/); resolve(readList()); await pending
  assert.equal(s.snapshot, null); assert.equal(s.busy, false)
  await s.refresh(async () => { throw new Error('corrupt-store') }); assert.equal(s.snapshot, null); assert.equal(s.error, 'corrupt-store')
})
