import { test } from 'node:test'
import assert from 'node:assert/strict'
import { DspDraft, effectKinds, filters, move, newEffect, parseDspJson, readChain, readNumbers, readPreview, starter } from './dsp.ts'
import type { PreviewRequest } from './dsp.ts'
import { copy } from './model.ts'
import { admission, previewFixture } from '../tests/dsp-fixture.ts'

function request(d: DspDraft): PreviewRequest { return { config: copy(d.config), frames: 4, iterations: 2, bypassed: d.bypassed, signal: { kind: 'impulse', amplitude: 0.25 } } }

test('chain import keeps ordered opaque channel IDs, all effects, raw fields and Float64 numeric domains', async () => {
  const d = new DspDraft(), config = starter('empty')
  config.format.sampleRateHz = 123457; config.format.channels = [' right ', 'L', 'aux-7']
  config.effects = effectKinds.map(newEffect)
  config.effects.push(...filters.map(filter => ({ name: 'same name allowed', enabled: false, effect: { kind: 'biquad' as const, filter, frequencyHz: 321.1234567890123, q: Math.SQRT1_2, gainDb: 0 } })))
  config.effects[0]!.effect = { kind: 'gain', gain: 1e20, invertPolarity: false }
  config.effects[5]!.effect = { kind: 'fir', coefficients: [1.0000000000000002, 0, -0.000000000000000123, 1e200, ...Array(4096).fill(0)] }
  config.futureField = { nested: ['retained'] }; config.format.futureFormat = 4; config.effects[1]!.futureEffect = true
  const calls: { path: string; body: unknown }[] = []
  await d.import({ post: async (path, body) => { calls.push({ path, body }); return { valid: true, precision: 'float64', admission } } }, JSON.stringify(config))
  assert.deepEqual(calls, [{ path: '/api/dsp/validate', body: { config, frames: 1 } }])
  assert.deepEqual(d.config, config)
  assert.deepEqual(JSON.parse(JSON.stringify(d.config)), config)
  assert.equal(d.report, null, 'validation cannot publish a hidden preview/timing')
})

test('invalid shape, nonfinite values and API domain rejection preserve the prior chain', async () => {
  const d = new DspDraft(), old = copy(d.config)
  let calls = 0
  const api = { post: async () => { calls++; throw new Error('Core: frequency must be below Nyquist') } }
  for (const patch of [{ format: { sampleRateHz: 0, channels: ['L'] } }, { format: { sampleRateHz: 4294967296, channels: ['L'] } }, { format: { sampleRateHz: 17, channels: ['L', 'L'] } }, { effects: [{ name: 'broken', enabled: true, effect: { kind: 'fir', coefficients: [] } }] }]) {
    await assert.rejects(d.import(api, JSON.stringify({ ...old, ...patch })))
    assert.deepEqual(d.config, old)
  }
  assert.throws(() => parseDspJson('{"x":1e999}'), /finite/)
  assert.throws(() => readNumbers('[0,"1"]'), /finite/)
  assert.equal(calls, 0)
  const bad = starter('empty'); bad.effects = [newEffect('biquad')]
  await assert.rejects(d.import(api, JSON.stringify(bad)), /Nyquist/)
  assert.equal(calls, 1); assert.deepEqual(d.config, old)
  await assert.rejects(d.import({ post: async () => ({ valid: false }) }, JSON.stringify(old)), /validate/)
  assert.deepEqual(d.config, old)
})

test('effect and channel reordering preserve every parameter, enable state and identity', () => {
  const c = starter('empty'); c.effects = effectKinds.map(newEffect)
  c.effects[1]!.enabled = false; const before = copy(c.effects)
  move(c.effects, 5, -1); move(c.effects, 0, 1)
  assert.deepEqual(c.effects, [before[1], before[0], before[2], before[3], before[5], before[4]])
  move(c.effects, 0, -1); move(c.effects, 5, 1)
  assert.equal(c.effects.length, 6)
  c.format.channels = ['R', 'L', 'rear']; move(c.format.channels, 2, -1)
  assert.deepEqual(c.format.channels, ['R', 'rear', 'L'])
  assert.doesNotThrow(() => readChain(c))
})

test('edits clear output and timing, and in-flight renders/imports cannot publish stale data', async () => {
  const d = new DspDraft(), req = request(d)
  await d.render({ post: async () => previewFixture(req) }, req)
  assert.ok(d.report)
  d.changed(); assert.equal(d.report, null)
  let resolve!: (value: unknown) => void
  const delayed = { post: () => new Promise<unknown>(r => { resolve = r }) }
  const pending = d.render(delayed, req)
  d.config.name = 'New edits'; d.changed(); resolve(previewFixture(req))
  assert.equal(await pending, false); assert.equal(d.report, null)
  const importing = d.import(delayed, JSON.stringify(starter('trim')))
  d.config.name = 'Keep this'; d.changed(); resolve({ valid: true, precision: 'float64', admission })
  assert.equal(await importing, false); assert.equal(d.config.name, 'Keep this')
})

test('response contract rejects incomplete or mismatched output; unknown timing is never a pass', () => {
  const req = request(new DspDraft()), response = previewFixture(req, { measured: false })
  assert.equal(readPreview(response, req).timing.measured, false)
  for (const patch of [{ output: [1] }, { precision: 'float32' }, { channels: ['FR', 'FL'] }, { meters: [] }, { meters: [{ peak: -1, rms: 1, overFullScaleSamples: 0 }, { peak: 1, rms: 1, overFullScaleSamples: 0 }] }, { timing: { ...response.timing, overBudget: true } }, { timing: { ...response.timing, meanMs: 0 } }]) {
    assert.throws(() => readPreview({ ...response, ...patch }, req))
  }
})

test('slow measurements and service failures keep every effect available for another render', async () => {
  const d = new DspDraft(); d.config.effects = effectKinds.map(newEffect)
  const req = request(d)
  await d.render({ post: async () => previewFixture(req, { slow: true }) }, req)
  assert.equal(d.report?.timing.overBudget, true); assert.deepEqual(d.config, req.config)
  await assert.rejects(d.render({ post: async () => { throw new Error('offline') } }, req), /offline/)
  assert.equal(d.report, null); assert.deepEqual(d.config, req.config)
  await d.render({ post: async () => previewFixture(req) }, req)
  assert.equal(readPreview(d.report, req).timing.overBudget, false)
})

test('Float64 JSON transport and draft snapshots retain signed zero and low bits without safe-integer narrowing', async () => {
  const { dspJson, copyDsp } = await import('./dsp-json.ts')
  const { Api } = await import('./api.ts')
  const value = { text: 'literal -0 and "quotes"', numbers: [-0, 1.0000000000000002, 1e100, 5e-324] }
  for (const pretty of [false, true]) assert.deepEqual(JSON.parse(dspJson(value, pretty)), value)
  assert.deepEqual(copyDsp(value), value)
  const bodies: unknown[] = []
  const api = new Api(async (_input, init) => {
    bodies.push(JSON.parse(String(init?.body)))
    return new Response('{}')
  })
  api.token = 'session'
  await api.post('/api/dsp/preview', value)
  assert.deepEqual(bodies, [value])
  const d = new DspDraft(), config = starter('fir')
  config.effects[0]!.effect = { kind: 'fir', coefficients: [-0, 1.0000000000000002] }
  await d.import({ post: async () => ({ valid: true, precision: 'float64', admission }) }, dspJson(config))
  assert.deepEqual(d.config, config)
})
