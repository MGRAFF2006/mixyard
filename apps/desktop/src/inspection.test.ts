import assert from 'node:assert/strict'
import { test } from 'node:test'
import { InspectionSession, MAX_INPUT_BYTES, readInspection, validateInput } from './inspection.ts'

const input = { configJson: '{"schemaVersion":2,"revision":12}', inventoryJson: '{"schemaVersion":2,"revision":34}' }
const response = () => ({ schemaVersion: 1, provenance: 'user-import', freshness: 'unverified', report: {
  schemaVersion: 1, mode: 'routing-inspection', configRevision: 12, inventoryRevision: 34,
  platform: 'linux', inventoryEvidence: 'simulated', submixes: 'not-implemented',
  nativeState: 'unknown', liveLevels: null, playerPauseState: 'unknown', captureState: 'unknown',
  planAvailable: true, preflightError: null as string | null, findings: [{ subject: 'permission', code: 'permission-unknown' }],
  capacity: [{ resource: 'output-routes', requested: 1, maximum: null, reason: 'capacity-unknown' }],
  sources: [{ configured: { id: 'mic', selector: { kind: 'mic', deviceUid: 'fixture:mic' } }, candidates: [],
    plannedResolution: 'missing', permission: 'unknown', captureState: 'unknown', reasons: ['source-missing'] }],
  mixes: [], sends: [], outputs: [{ configured: { id: 'headphones', mix: 'monitor', target: { kind: 'device', deviceUid: 'fixture:phones' }, gain: 1, mute: false, allowMicMonitor: true },
    mix: null, candidates: [], plannedResolution: 'missing', consumer: 'unknown', nativeState: 'unknown', reasons: ['output-missing'],
    paths: [{ source: 'mic', configuredSend: { source: 'mic', mix: 'monitor', gain: 0, mute: true }, intendedPotentialSharing: true, excluded: false, plannedEffectiveCoefficient: 0 as number | null, appliedSharing: 'unknown', reasons: ['send-muted', 'send-zero'] }] }],
  plan: { schemaVersion: 2, mode: 'dry-run', configRevision: 12, inventoryRevision: 34, inventoryEvidence: 'simulated', platform: 'linux', mixes: [], outputs: [], diagnostics: [] } as object | null,
} })
const read = () => readInspection(JSON.stringify(response()))

test('import boundary rejects malformed, discovery v1, oversized UTF-8 and imprecise revisions', () => {
  assert.equal(validateInput(input), input)
  for (const configJson of ['{', '[]', '{"schemaVersion":1,"revision":0}', '{"schemaVersion":2,"revision":9007199254740992}', 'é'.repeat(MAX_INPUT_BYTES / 2 + 1)]) {
    assert.throws(() => validateInput({ ...input, configJson }))
  }
})

test('typed report retains muted/zero potential intent, revisions and unknown live state', () => {
  const result = read()
  assert.equal(result.configRevision, 12)
  assert.equal(result.inventoryRevision, 34)
  assert.equal(result.inventoryEvidence, 'simulated')
  assert.equal(result.liveLevels, null)
  assert.equal(result.outputs[0]!.paths[0]!.intendedPotentialSharing, true)
  assert.deepEqual(result.outputs[0]!.paths[0]!.reasons, ['send-muted', 'send-zero'])
  assert.equal(result.outputs[0]!.paths[0]!.plannedEffectiveCoefficient, 0)
  assert.equal(result.outputs[0]!.paths[0]!.appliedSharing, 'unknown')
})

test('unsupported schemas, fabricated live claims and rejected-plan coefficients fail closed', () => {
  const mutations: Array<(v: ReturnType<typeof response>) => void> = [
    v => { v.schemaVersion = 2 }, v => { v.provenance = 'live' }, v => { v.freshness = 'current' },
    v => { v.report.inventoryEvidence = 'verified' }, v => { v.report.nativeState = 'active' },
    v => { v.report.sources[0]!.permission = 'granted' }, v => { v.report.outputs[0]!.consumer = 'connected' },
    v => { v.report.outputs[0]!.paths[0]!.appliedSharing = 'true' }, v => { v.report.findings[0]!.code = 'new-unknown-reason' },
    v => { v.report.configRevision = 2 ** 53 }, v => { v.report.plan = null },
    v => { v.report.planAvailable = false; v.report.plan = null; v.report.preflightError = 'Headroom rejected' },
  ]
  for (const mutate of mutations) {
    const v = response(); mutate(v)
    assert.throws(() => readInspection(JSON.stringify(v)))
  }
  assert.throws(() => readInspection(response()))
  const rejected = response()
  rejected.report.planAvailable = false; rejected.report.plan = null; rejected.report.preflightError = 'Headroom rejected'
  rejected.report.outputs[0]!.paths[0]!.plannedEffectiveCoefficient = null
  assert.equal(readInspection(JSON.stringify(rejected)).outputs[0]!.paths[0]!.plannedEffectiveCoefficient, null)
})

test('input changes, rejection, errors and dispose clear old results and suppress late responses', async () => {
  const session = new InspectionSession()
  await session.run(input, async () => read())
  assert.ok(session.result)
  let resolve!: (value: ReturnType<typeof read>) => void
  const pending = session.run(input, () => new Promise(done => { resolve = done }))
  assert.equal(session.busy, true)
  assert.equal(session.result, null)
  session.invalidate()
  resolve(read()); await pending
  assert.equal(session.result, null)
  await session.run(input, async () => read())
  await session.run({ ...input, configJson: '{}' }, async () => { throw new Error('Must not invoke') })
  assert.equal(session.result, null)
  assert.match(session.error, /schema 2/)
  await session.run(input, async () => { throw new Error('native unavailable') })
  assert.equal(session.result, null)
  assert.equal(session.error, 'native unavailable')
  const disposed = session.run(input, () => new Promise(done => { resolve = done }))
  session.dispose(); resolve(read()); await disposed
  assert.equal(session.result, null)
})

test('a second request is rejected without invoking and invalidates the first result', async () => {
  const session = new InspectionSession()
  let resolve!: (value: ReturnType<typeof read>) => void
  const pending = session.run(input, () => new Promise(done => { resolve = done }))
  await session.run(input, async () => { assert.fail('overlapping invoke') })
  assert.equal(session.busy, true)
  assert.match(session.error, /already running/)
  resolve(read()); await pending
  assert.equal(session.result, null)
  assert.equal(session.busy, false)
})
