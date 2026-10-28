import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Api } from './api.ts'
import { copyDsp, dspJson } from './dsp-json.ts'
import { SituationSession } from './situation-session.ts'
import { buildPreset, deleteNode, emptyPreset, emptyRequest, graphIssues, parseSituation, presetProblems, readEvaluation, readRequest, renameNode } from './situations.ts'
import type { Evaluation, SituationRequest } from './situations.ts'

function headphones(): SituationRequest { return buildPreset({ ...emptyPreset(), device: 'device/opaque Ω', profile: 'p-UUID/1', normalProfile: 'p-UUID/2', output: 'speakers/route' }, emptyRequest()) }
function result(r: SituationRequest, patch: Partial<Evaluation> = {}) { return { mode: 'simulation', evaluation: { schemaVersion: 1, token: { graphId: r.graph.id, graphRevision: r.graph.revision, snapshotRevision: r.snapshot.revision, epoch: r.snapshot.epoch, evaluatedAtMs: r.nowMs, context: copyDsp(r.context) }, action: { kind: 'no-change' }, terminal: 'safe', observations: [], trace: [{ kind: 'decision', node: 'when', result: 'unknown', next: 'safe' }, { kind: 'terminal', node: 'safe' }], traceTruncated: false, audioApplied: false, ...patch } } }

test('exact node identity rename relinks every branch and root, deletion blocks references, all nodes detect cycles/missing targets', () => {
  const r = headphones(), before = dspJson(r.graph)
  const renamed = renameNode(r.graph, 'safe', ' SAFE / Ω ')
  assert.equal(renamed.nodes[0]?.kind === 'decision' && renamed.nodes[0].onUnknown, ' SAFE / Ω ')
  assert.equal(dspJson(r.graph), before, 'rename is atomic and does not mutate the original')
  assert.throws(() => renameNode(renamed, 'active', 'normal'), /already exists/)
  assert.throws(() => deleteNode(renamed, ' SAFE / Ω '), /Relink before deleting/)
  const root = renameNode(renamed, 'when', 'root/EXACT'); assert.equal(root.root, 'root/EXACT')
  root.nodes.push({ id: 'unused', kind: 'terminal', action: { kind: 'no-change' } })
  assert.equal(deleteNode(root, 'unused').nodes.length, 4)
  const n = root.nodes[0]!; assert.equal(n.kind, 'decision'); if (n.kind !== 'decision') throw Error('fixture')
  n.onTrue = n.id; assert.match(graphIssues(root).join(), /cycle/)
  n.onTrue = 'missing'; assert.match(graphIssues(root).join(), /missing branch target/)
  n.onTrue = 'active'; root.nodes.push({ ...n, id: 'unreachable', onTrue: 'unreachable' }); assert.match(graphIssues(root).join(), /cycle/)
})

test('presets require actual IDs and explicit declarations; unknown stays a configurable independent branch', () => {
  const p = emptyPreset(); assert.ok(presetProblems(p).includes('Exact device UID')); assert.throws(() => buildPreset(p, emptyRequest()), /incomplete/)
  const r = headphones(), when = r.graph.nodes[0]!; assert.equal(when.kind, 'decision')
  if (when.kind !== 'decision') throw Error('fixture')
  assert.deepEqual(when.condition, { kind: 'fact', key: { name: 'device.connected' }, predicate: { op: 'contains', value: 'device/opaque Ω' } })
  assert.equal(when.onFalse, 'normal'); assert.equal(when.onUnknown, 'safe')
  assert.deepEqual(r.graph.nodes[1], { id: 'active', kind: 'terminal', action: { kind: 'select', profileId: 'p-UUID/1', overrides: [{ scope: 'output', output: 'speakers/route', mute: true }] } })
  assert.equal(r.snapshot.facts.length, 0, 'device presence is never fabricated')
  const monitor = { ...emptyPreset(), kind: 'monitor' as const, device: 'interface/1', source: 'mic/exact', mix: 'monitor/exact' }
  assert.throws(() => buildPreset(monitor, emptyRequest()), /Explicit hardware-monitor declaration/)
  const declared = buildPreset({ ...monitor, declared: true }, emptyRequest())
  assert.deepEqual(declared.graph.nodes[1], { id: 'active', kind: 'terminal', action: { kind: 'select', overrides: [{ scope: 'send', source: 'mic/exact', mix: 'monitor/exact', mute: true }] } })
  assert.equal(declared.snapshot.facts[0]?.evidence, 'user-declared'); assert.equal(declared.snapshot.facts[0]?.provider, 'user-simulation')
  const customSafe = buildPreset({ ...monitor, declared: true, unknownProfile: 'safety/profile' }, emptyRequest())
  assert.deepEqual(customSafe.graph.nodes[3], { id: 'safe', kind: 'terminal', action: { kind: 'select', profileId: 'safety/profile', overrides: [] } })
})

test('application/time/power preset remains composable and civil time explicitly includes provider timezone', () => {
  const r = buildPreset({ ...emptyPreset(), kind: 'applications', apps: ['app:opaque', 'app:second'], profile: 'p/active', normalProfile: 'p/normal', power: 'battery', useTime: true, timezone: 'Europe/Berlin', startMinute: 540, endMinute: 1020 }, emptyRequest())
  assert.deepEqual(readRequest(r), r)
  assert.deepEqual(r.graph.nodes[0]?.kind === 'decision' && r.graph.nodes[0].condition, { kind: 'all', conditions: [ { kind: 'fact', key: { name: 'application.running' }, predicate: { op: 'contains-all', values: ['app:opaque', 'app:second'] } }, { kind: 'fact', key: { name: 'power.source' }, predicate: { op: 'eq', value: { type: 'text', value: 'battery' } } }, { kind: 'fact', key: { name: 'civil.timezone' }, predicate: { op: 'eq', value: { type: 'text', value: 'Europe/Berlin' } } }, { kind: 'fact', key: { name: 'civil.minute-of-day' }, predicate: { op: 'range', min: 540, max: 1020 } } ] })
})

test('full Request round trip preserves every parameter/raw field and finite Float64 bits; metadata never narrows silently', () => {
  const r = headphones(), n = r.graph.nodes[0]!; if (n.kind !== 'decision') throw Error('fixture')
  r.catalog.push({ name: 'custom.number', valueType: 'number', scope: 'global', maxAgeMs: 1234, evidencePolicy: 'any', extra: { opaque: true } })
  n.condition = { kind: 'all', conditions: [ { kind: 'not', condition: { kind: 'fact', key: { name: 'custom.number', rawKey: 'preserve' }, predicate: { op: 'range', min: -0, max: 9007199254740994 }, rawCondition: 0.9999999999999999 } }, { kind: 'any', conditions: [{ kind: 'fact', key: { name: 'device.connected' }, predicate: { op: 'contains-any', values: [' exact ID ', 'Ω'] } }] } ] }
  r.raw = { deep: [{ signed: -0, low: 1.0000000000000002 }] }; r.graph.rawGraph = { disabled: false }; r.context.rawContext = 'retained'
  r.snapshot.facts.push({ key: { name: 'custom.number' }, status: 'known', value: { type: 'number', value: -0, extraValue: 9007199254740994 }, provider: 'supplied', evidence: 'fixture', observedAtMs: 0, validForMs: 20, completeness: 'complete', extraFact: { low: 1.0000000000000002 } })
  const terminal = r.graph.nodes[1]!; if (terminal.kind !== 'terminal' || terminal.action.kind !== 'select') throw Error('fixture'); terminal.action.overrides[0]!.gain = 0.9999999999999999
  const text = dspJson(r, true); assert.match(text, /"signed": -0/); assert.deepEqual(parseSituation(text), r); assert.equal(dspJson(copyDsp(readRequest(r))), dspJson(r))
  const bad = copyDsp(r); bad.context.storeRevision = 9007199254740994; assert.throws(() => readRequest(bad), /exact nonnegative integer/)
  bad.context.storeRevision = 0; bad.nowMs = 1.5; assert.throws(() => readRequest(bad), /exact nonnegative integer/)
  const unsafe = copyDsp(r); unsafe.raw = NaN; assert.throws(() => readRequest(unsafe), /finite/)
})

test('simulation unknown observations and negatives are sent intact and core reasons/trace evidence are used verbatim', async () => {
  const r = headphones(), n = r.graph.nodes[0]!; if (n.kind !== 'decision') throw Error('fixture')
  n.condition = { kind: 'not', condition: { kind: 'fact', key: { name: 'device.connected' }, predicate: { op: 'contains', value: 'device/opaque Ω' } } }
  const fact = { key: { name: 'device.connected' }, status: 'known' as const, value: { type: 'text-set' as const, value: ['device/opaque Ω'] }, provider: 'claim', evidence: 'platform-observed', observedAtMs: 0, validForMs: 5, completeness: 'partial' as const, reason: 'set incomplete' }
  r.snapshot.facts.push(fact); r.nowMs = 10
  const e = readEvaluation(result(r, { observations: [fact], trace: [{ kind: 'fact', node: 'when', detail: { key: fact.key, predicate: { op: 'contains', value: 'device/opaque Ω' }, result: 'unknown', observationIndex: 0, ageMs: 10, fresh: false, unknownReason: 'stale-observation' } }, { kind: 'decision', node: 'when', result: 'unknown', next: 'safe' }, { kind: 'terminal', node: 'safe' }], traceTruncated: true }), r)
  assert.equal(e.traceTruncated, true); assert.equal(e.observations.length, 1); assert.equal(e.trace[1]?.kind === 'decision' && e.trace[1].result, 'unknown')
  const requests: unknown[] = [], session = new SituationSession({ post: async (_, request) => { requests.push(request); return result(r) } }); session.request = r; await session.evaluate()
  assert.deepEqual(requests[0], r); assert.ok(session.evaluation); assert.equal(n.onUnknown, 'safe', 'unknown is not the false branch, including Not')
})

test('edits and invalid text drafts discard in-flight evaluation, validation, imports and clear previous trace', async () => {
  let resolve!: (v: unknown) => void
  const s = new SituationSession({ post: async () => new Promise(r => { resolve = r }) }); s.request = headphones()
  const evaluation = s.evaluate(), old = copyDsp(s.request); s.edit(r => { r.graph.name = 'Later edit' }); resolve(result(old)); await evaluation; assert.equal(s.evaluation, null)
  const validation = s.validate(); s.invalidate(); resolve({ valid: true }); await validation; assert.equal(s.validated, false)
  const importing = s.import(dspJson(headphones())); s.edit(r => { r.graph.name = 'Keep me' }); resolve({ valid: true }); assert.equal(await importing, false); assert.equal(s.request.graph.name, 'Keep me')
})

test('imports replace only after full core validation/evaluation, failures retain all prior data and permit retry', async () => {
  const calls: string[] = []; let fail = true
  const s = new SituationSession({ post: async (path, body) => { calls.push(path); if (path.endsWith('/validate')) return { valid: true }; if (fail) throw Error('authoritative unsupported-field'); return result(body as SituationRequest) } }), before = dspJson(s.request)
  assert.equal(await s.import(dspJson(headphones())), false); assert.match(s.error, /unsupported-field/); assert.equal(dspJson(s.request), before)
  fail = false; assert.equal(await s.import(dspJson(headphones())), true); assert.deepEqual(calls, ['/api/automation/validate', '/api/automation/evaluate', '/api/automation/validate', '/api/automation/evaluate']); assert.equal(s.request.graph.root, 'when'); assert.equal(s.evaluation, null)
})

test('automation transport preserves signed zero and finite low bits without altering legacy routing JSON', async () => {
  const requests: string[] = [], api = new Api(async (_url, init) => { requests.push(String(init?.body)); return new Response(requests.length === 1 ? '{"apiVersion":1,"csrfToken":"x","audioAvailable":false}' : '{"valid":true}') })
  await api.connect(); await api.post('/api/automation/validate', { value: -0, low: 1.0000000000000002, large: 9007199254740994 }); await api.post('/api/validate', { value: -0 })
  assert.equal(requests[1], '{"value":-0,"low":1.0000000000000002,"large":9007199254740994}'); assert.equal(requests[2], '{"value":0}')
})

test('offline context retries keep edits and report provider availability without inventing complete facts', async () => {
  let offline = true
  const s = new SituationSession({ post: async () => { if (offline) throw Error('offline'); return { mode: 'read-only-live-context', snapshot: { schemaVersion: 1, revision: 4, epoch: 'live', facts: [] }, nowMs: 4, catalog: emptyRequest().catalog, providers: [{ provider: 'device', status: 'partial', reason: 'More than 256 device identities; no known truncated set', extra: true }] } } })
  s.edit(r => { r.graph.name = 'Local work' }); const before = dspJson(s.request); await s.collect(); assert.match(s.liveError, /offline/); assert.equal(dspJson(s.request), before)
  offline = false; await s.collect(); assert.equal(s.live?.providers[0]?.status, 'partial'); assert.equal(dspJson(s.request), before); s.useLive(); assert.equal(s.request.graph.name, 'Local work'); assert.equal(s.request.snapshot.epoch, 'live'); assert.deepEqual(s.request.snapshot.facts, [])
})

test('resource, type and reduction rejections explain invalid configuration without coercion or cutting', () => {
  const r = headphones(), a = r.graph.nodes[1]!; if (a.kind !== 'terminal' || a.action.kind !== 'select') throw Error('fixture')
  a.action.overrides[0]!.mute = false; assert.throws(() => readRequest(r), /never unmute/)
  a.action.overrides[0]!.mute = true; a.action.overrides[0]!.gain = 1.01; assert.throws(() => readRequest(r), /between 0 and 1/)
  delete a.action.overrides[0]!.gain; r.graph.nodes = Array.from({ length: 257 }, (_, i) => ({ id: `n${i}`, kind: 'terminal', action: { kind: 'no-change' } })); assert.throws(() => readRequest(r), /exceeds 256/)
})

test('ambiguous duplicate JSON properties are rejected before parsing can silently replace a branch or raw field', () => {
  const text = dspJson(headphones())
  assert.throws(() => parseSituation(text.replace('"onUnknown":"safe"', '"onUnknown":"active","onUnknown":"safe"')), /Duplicate JSON field: onUnknown/)
  assert.throws(() => parseSituation(text.replace('"id":"situations"', '"id":"first","\\u0069d":"situations"')), /Duplicate JSON field: id/)
  const request = headphones(); request.extra = { nested: [{ quote: 'text with \\" { } : [ ]', different: 'x' }], exact: 'okay' }
  assert.deepEqual(parseSituation(dspJson(request)), request)
})

test('core Option null and omitted fields survive imports without materializing or dropping values', () => {
  const r = headphones(), decision = r.graph.nodes[0]!, terminal = r.graph.nodes[1]!
  if (decision.kind !== 'decision' || decision.condition.kind !== 'fact' || terminal.kind !== 'terminal' || terminal.action.kind !== 'select') throw Error('fixture')
  decision.condition.key.subject = null
  terminal.action.profileId = null
  terminal.action.overrides[0]!.gain = null
  r.snapshot.facts = [{ key: { name: 'device.connected', subject: null }, status: 'unknown', value: null, provider: 'claim', evidence: 'fixture', observedAtMs: null, validForMs: 0, completeness: 'partial', reason: null }]
  assert.equal(dspJson(parseSituation(dspJson(r))), dspJson(r))
})

test('graph/catalog policy and snapshot fact edits advance their owned revisions while failed renames stay atomic', () => {
  const s = new SituationSession({ post: async () => ({}) }); s.request = headphones()
  s.edit(r => { r.catalog[0]!.maxAgeMs = 1234 }); assert.equal(s.request.graph.revision, 1)
  s.edit(r => { r.snapshot.facts.push({ key: { name: 'device.connected' }, status: 'unknown', provider: 'simulation', evidence: 'fixture', validForMs: 0, completeness: 'partial' }) }); assert.equal(s.request.snapshot.revision, 1)
  const before = dspJson(s.request); s.edit(r => { r.graph = renameNode(r.graph, 'when', 'active') }); assert.match(s.error, /already exists/); assert.equal(dspJson(s.request), before)
})
