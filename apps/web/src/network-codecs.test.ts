import { test } from 'node:test'
import assert from 'node:assert/strict'
import { CodecWorkspace, codecSignature, configFromForm, formFromConfig, parseCodecConfig, parseCodecJson, readCodecConfig, readCodecEvidence } from './network-codecs.ts'
import { codecBinding, codecFixture, rawIntent } from '../tests/network-codec-fixture.ts'
import type { CodecBinding, CodecConfig, CodecEvidence } from './network-codecs.ts'
const copy = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T

test('frozen config keeps full-u64 strings and every explicit encoding boundary', () => {
  const cases: CodecConfig[] = [rawIntent, { ...rawIntent, encoding: { mode: 'zstd', level: 3 } }, ...[6000, 512000].flatMap(bitrate => [0, 10].map(complexity => ({ ...rawIntent, encoding: { mode: 'opus' as const, bitrate, complexity, vbr: false, conversion: 'normalized-f32-clamp-v1' as const }, allowLossy: true })))]
  for (const config of cases) {
    assert.deepEqual(parseCodecConfig(JSON.stringify(config)), config)
    assert.deepEqual(configFromForm(formFromConfig(config)), config)
    assert.equal(Object.keys(config).length, 5)
  }
  for (const id of ['0', '01', '-1', '1.0', '1e3', '18446744073709551616', ' 1', '١']) assert.throws(() => readCodecConfig({ ...rawIntent, streamId: id }))
  for (const encoding of [{ mode: 'raw', level: 1 }, { mode: 'zstd', level: 0 }, { mode: 'zstd', level: 4 }, { mode: 'zstd', level: 1.5 }, { mode: 'flac' }, { mode: 'opus', bitrate: 5999, complexity: 1, vbr: true, conversion: 'normalized-f32-clamp-v1' }, { mode: 'opus', bitrate: 512001, complexity: 1, vbr: true, conversion: 'normalized-f32-clamp-v1' }]) assert.throws(() => readCodecConfig({ ...rawIntent, encoding, allowLossy: true }))
  const opus = cases[2]!
  for (const encoding of [{ mode: 'raw' }, { mode: 'zstd', level: 1 }, { mode: 'zstd', level: 3 }]) {
    assert.throws(() => readCodecConfig({ ...rawIntent, encoding, allowLossy: true }), /Raw and Zstd require/)
    const s = new CodecWorkspace(); s.selected.form = formFromConfig({ ...rawIntent, encoding, allowLossy: true } as CodecConfig)
    assert.equal(s.validation.config, null)
    assert.throws(() => configFromForm(s.selected.form), /Raw and Zstd require/)
  }
  for (const patch of [{ allowLossy: false }, { allowLossy: 'true' }, { streamId: 1 }, { generation: '0' }, { version: 2 }, { extra: true }, { encoding: { ...opus.encoding, vbr: 1 } }, { encoding: { ...opus.encoding, complexity: 11 } }, { encoding: { ...opus.encoding, conversion: 'automatic' } }]) assert.throws(() => readCodecConfig({ ...opus, ...patch }))
})

test('ambiguous JSON, rounded integer lexemes, arrays and unknown fields cannot become valid intent', () => {
  const raw = JSON.stringify(rawIntent)
  for (const input of [raw.replace('"version":1', '"version":1,"version":1'), raw.replace('"version":1', '"version":1,"\\u0076ersion":1'), raw.replace('"mode":"raw"', '"mode":"raw","mode":"raw"'), raw.replace('"version":1', '"version":1.00000000000000001'), raw.replace('"version":1', '"version":1e0'), raw.replace('"version":1', '"version":-0'), raw.replace('"raw"', '"raw","credential":"hidden"'), `[${raw}]`, raw + 'false', raw.replace('false', '0'), raw.replace('"encoding":{"mode":"raw"}', '"encoding":[]')]) assert.throws(() => parseCodecConfig(input))
  assert.throws(() => parseCodecJson('['.repeat(30) + '0' + ']'.repeat(30)), /nesting/)
  assert.throws(() => parseCodecJson('"' + 'x'.repeat(131072) + '"'), /exceeds/)
})

test('raw invalid fields, imported text and array input survive draft switches and portable backup without authority', () => {
  const s = new CodecWorkspace(); assert.equal(s.dirty, false)
  s.setBinding(codecBinding); s.selected.form = { ...formFromConfig(rawIntent), mode: 'opus', bitrate: '6000.00000000000001', complexity: '-' }; s.bind()
  s.importText = '[{"broken":'; const first = s.selectedId
  assert.equal(s.validation.config, null); assert.equal(s.dirty, true)
  s.add({ ...formFromConfig(rawIntent), generation: '2', level: 'not a number' })
  s.select(first); assert.equal(s.selected.form.bitrate, '6000.00000000000001'); assert.equal(s.importText, '[{"broken":')
  const backup = s.backup(); assert.equal(backup.includes('authorityId'), false); assert.equal(backup.includes('A-boot'), false)
  const restored = new CodecWorkspace(); restored.restore(backup)
  assert.equal(restored.drafts.length, 3); assert.deepEqual(restored.drafts.slice(1).map(d => d.form), s.drafts.map(d => d.form)); assert.equal(restored.importText, s.importText)
  assert.ok(restored.drafts.every(d => d.owner === null)); assert.equal(restored.evidence, null); assert.equal(restored.dirty, true)
  const previous = s.backup(); assert.throws(() => s.importIntent()); assert.equal(s.backup(), previous)
  s.importText = JSON.stringify(rawIntent); s.importIntent(); assert.deepEqual(s.validation.config, rawIntent); assert.equal(s.selected.owner, null); assert.equal(s.importText, JSON.stringify(rawIntent))
})

test('qualified reports preserve exact provenance, request, cursor, metrics and missing evidence reasons', () => {
  const fixture = codecFixture(), report = readCodecEvidence(JSON.stringify(fixture), codecBinding, rawIntent)
  assert.deepEqual(report, fixture)
  // Object property order does not change the request identity; channel order does.
  const reversed = JSON.parse(JSON.stringify(fixture.capability.request, Object.keys(fixture.capability.request).reverse()))
  reversed.encoding = { mode: 'raw' }; fixture.capability.request = reversed
  assert.equal(readCodecEvidence(fixture, codecBinding, rawIntent).capability.available, true)
  const noMeasurements = codecFixture(); noMeasurements.capability.measurements = null; noMeasurements.capability.measurementUnavailableReason = 'No completed operation'; noMeasurements.timeline = null
  assert.equal(readCodecEvidence(noMeasurements, codecBinding, rawIntent).timeline, null)
  for (const state of ['send-operation-in-flight', 'different-plan-admitted'] as const) {
    const f = codecFixture(); f.direction = 'send'; f.capability.agreementState = state
    f.capability.measurements = null; f.capability.measurementUnavailableReason = 'No completed exact-plan operation'; f.timeline = null
    assert.equal(readCodecEvidence(f, codecBinding, rawIntent).capability.agreementState, state)
    f.direction = 'receive'; assert.throws(() => readCodecEvidence(f, codecBinding, rawIntent), /Agreement direction mismatch/)
  }
  const edits: ((f: ReturnType<typeof codecFixture>) => void)[] = [f => { f.binding.revision++ }, f => { f.capability.request = { ...rawIntent, generation: '2' } }, f => { f.originalSpec.stream = '2' }, f => { f.capability.measurements!.cpuUnavailableReason = null }, f => { f.capability.maxWorkWallNanos = Number.MAX_SAFE_INTEGER + 1 }, f => { f.timeline!.nextSourceSampleFrame = '01' }, f => { f.timeline!.bitPreserving = false }, f => { f.timeline!.sourceSpec = { ...f.originalSpec, channels: ['FR', 'FL'] } }, f => { f.capability.available = false }, f => { f.direction = 'send' }]
  for (const edit of edits) { const f = copy(codecFixture()); edit(f); assert.throws(() => readCodecEvidence(f, codecBinding, rawIntent)) }
})

function pending() { let resolve!: (value: unknown) => void; const promise = new Promise<unknown>(done => { resolve = done }); return { promise, resolve } }
function workspace() { const s = new CodecWorkspace(); s.setBinding(codecBinding); s.selected.form = formFromConfig(rawIntent); s.bind(); return s }

test('disconnect, revocation, authority/revision/epoch changes, edit and target switch invalidate in-flight probes without retargeting', async () => {
  for (const invalidate of [
    (s: CodecWorkspace) => s.setBinding(null),
    ...(['deviceId', 'targetId', 'authorityId', 'revision', 'epoch'] as const).map(key => (s: CodecWorkspace) => s.setBinding({ ...codecBinding, [key]: typeof codecBinding[key] === 'string' ? 'other' : 99 } as CodecBinding)),
    (s: CodecWorkspace) => { s.selected.form.generation = '2'; s.changed() },
    (s: CodecWorkspace) => s.setActive(false),
    (s: CodecWorkspace) => s.add(),
  ]) {
    const s = workspace(), held = pending(); let aborted = false, captured: unknown
    const task = s.probe({ probe: async (binding, request, signal) => { captured = copy({ binding, request }); signal.addEventListener('abort', () => { aborted = true }); return held.promise } })
    invalidate(s); assert.equal(s.evidence, null); assert.equal(s.busy, false); assert.equal(aborted, true)
    held.resolve(codecFixture()); await task
    assert.equal(s.evidence, null); assert.deepEqual(captured, { binding: codecBinding, request: rawIntent })
  }
})

test('imports and reconnected management roles cannot restore codec authority; only a fresh explicit read is admitted', async () => {
  const s = workspace(); let calls = 0
  const host = { probe: async (binding: Readonly<CodecBinding>, request: Readonly<CodecConfig>) => { calls++; return codecFixture(binding, request) } }
  await s.probe(host); assert.equal(s.evidence?.evidenceKind, 'fixture')
  s.setBinding(null); s.setBinding({ ...codecBinding, epoch: 2 }); await s.probe(host); assert.equal(calls, 1)
  assert.equal(s.selected.owner?.epoch, 1); assert.equal(s.evidence, null)
  s.bind(); await s.probe(host); assert.equal(calls, 2); assert.equal((s.evidence as CodecEvidence | null)?.binding.epoch, 2)
  const forms = s.backup(); assert.equal(forms.includes('available'), false)
  const fresh = new CodecWorkspace(); fresh.restore(forms); fresh.setBinding(codecBinding); await fresh.probe(host); assert.equal(calls, 2)
  assert.equal(codecSignature(s.validation.config), codecSignature(rawIntent))
})
