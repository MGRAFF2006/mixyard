import { test } from 'node:test'
import assert from 'node:assert/strict'
import { addChannel, alignment, balance, channelLevel, matrixSummary, quantizeDelay, removeChannel, renameChannel, routing } from './channels.ts'
import { DspDraft, effectKinds, move, newEffect, readChain, starter } from './dsp.ts'
import { copyDsp, dspJson } from './dsp-json.ts'
import { admission } from '../tests/dsp-fixture.ts'

const ids = [' tap / A ', 'rear-7', 'aux:ø']
test('beginner operations append exact independent lane processing with explicit pair balance law', () => {
  const c = starter('fir'), original = copyDsp(c.effects)
  c.format.channels = [...ids]
  c.effects.push(channelLevel(ids[0]!, -6, true), balance(ids, ids[2]!, ids[0]!, -25), routing(ids, ids[1]!, ids[2]!, 'swap'), routing(ids, ids[0]!, ids[1]!, 'copy'), alignment(ids[0]!, 48), alignment(ids[1]!, 96))
  assert.deepEqual(c.effects.slice(0, 1), original, 'shortcuts must not reinterpret or replace existing stages')
  assert.deepEqual(c.effects[1], { name: 'Level / polarity " tap / A "', enabled: true, channels: [ids[0]], effect: { kind: 'gain', gain: 10 ** (-6 / 20), invertPolarity: true } })
  assert.deepEqual(c.effects[2]!.effect, { kind: 'channel-matrix', routes: [
    { output: ids[0], inputs: [{ channel: ids[0], gain: 0.75 }] }, { output: ids[1], inputs: [{ channel: ids[1], gain: 1 }] }, { output: ids[2], inputs: [{ channel: ids[2], gain: 1 }] },
  ] })
  assert.deepEqual(c.effects[3]!.effect, { kind: 'channel-matrix', routes: [
    { output: ids[0], inputs: [{ channel: ids[0], gain: 1 }] }, { output: ids[1], inputs: [{ channel: ids[2], gain: 1 }] }, { output: ids[2], inputs: [{ channel: ids[1], gain: 1 }] },
  ] })
  assert.deepEqual(c.effects[4]!.effect, { kind: 'channel-matrix', routes: [
    { output: ids[0], inputs: [{ channel: ids[0], gain: 1 }] }, { output: ids[1], inputs: [{ channel: ids[0], gain: 1 }] }, { output: ids[2], inputs: [{ channel: ids[2], gain: 1 }] },
  ] })
  assert.deepEqual(c.effects.slice(-2).map(e => ({ channels: e.channels, effect: e.effect })), [
    { channels: [ids[0]], effect: { kind: 'delay', frames: 48, wet: 1, dry: 0, feedback: 0 } }, { channels: [ids[1]], effect: { kind: 'delay', frames: 96, wet: 1, dry: 0, feedback: 0 } },
  ])
  assert.doesNotThrow(() => readChain(c))
  assert.throws(() => balance(ids, ids[0]!, ids[0]!, 0), /different/)
  assert.throws(() => alignment(ids[0]!, 1.5), /Fractional/)
})

test('selectors apply to every existing effect, null/missing mean all, invalid references reject before validation', async () => {
  const d = new DspDraft(), c = starter('empty'); c.format.channels = [...ids]
  c.effects = effectKinds.filter(kind => kind !== 'channel-matrix').map(kind => ({ ...newEffect(kind), channels: [ids[2]!, ids[0]!] }))
  assert.deepEqual(readChain(c), c)
  c.effects[0]!.channels = null; assert.doesNotThrow(() => readChain(c))
  delete c.effects[0]!.channels; assert.doesNotThrow(() => readChain(c))
  let calls = 0
  for (const channels of [[], [ids[0], ids[0]], ['missing'], [1]]) {
    const invalid = copyDsp(c); invalid.effects[0]!.channels = channels as string[]
    await assert.rejects(d.import({ post: async () => { calls++; return {} } }, dspJson(invalid)))
  }
  assert.equal(calls, 0)
})

test('custom matrix retains exact signed coefficients, silence, raw fields and ordered identities through import/export', async () => {
  const c = starter('empty'); c.format.channels = [...ids]
  const matrix = newEffect('channel-matrix', ids); c.effects.push(matrix)
  if (matrix.effect.kind !== 'channel-matrix') throw new Error('matrix required')
  matrix.extra = { retained: 'yes' }
  matrix.effect.routes[0]!.inputs = [{ channel: ids[1]!, gain: -0 }, { channel: ids[2]!, gain: -1.0000000000000002, future: [4] }]
  matrix.effect.routes[1]!.inputs = []; matrix.effect.routes[2]!.inputs = [{ channel: ids[0]!, gain: 1e100 }]
  assert.equal(matrixSummary(matrix.effect.routes), 'Custom matrix — coefficients preserved')
  const d = new DspDraft()
  await d.import({ post: async () => ({ valid: true, precision: 'float64', admission }) }, dspJson(c))
  assert.deepEqual(JSON.parse(dspJson(d.config)), c)
  move(d.config.format.channels, 2, -1)
  assert.deepEqual(d.config.effects, c.effects, 'reordering lanes cannot retarget matrix IDs')
  assert.doesNotThrow(() => readChain(d.config))
  matrix.channels = null
  assert.doesNotThrow(() => readChain(c), 'null has the same meaning as an omitted selector in Rust')
  for (const mutate of [
    (e: typeof matrix) => { e.channels = [ids[0]!] },
    (e: typeof matrix) => { if (e.effect.kind === 'channel-matrix') e.effect.routes.pop() },
    (e: typeof matrix) => { if (e.effect.kind === 'channel-matrix') e.effect.routes[1]!.output = ids[0]! },
    (e: typeof matrix) => { if (e.effect.kind === 'channel-matrix') e.effect.routes[0]!.inputs.push({ channel: ids[1]!, gain: 0.1 }) },
    (e: typeof matrix) => { if (e.effect.kind === 'channel-matrix') e.effect.routes[0]!.inputs[0]!.channel = 'missing' },
  ]) {
    const invalid = copyDsp(c); mutate(invalid.effects[0]!); assert.throws(() => readChain(invalid))
  }
})

test('explicit rename updates all same-identity references; blocked format edits are atomic', () => {
  const c = starter('empty'); c.format.channels = [...ids]
  c.effects = [alignment(ids[0]!, 5), newEffect('channel-matrix', ids)]
  const before = copyDsp(c)
  assert.throws(() => removeChannel(c, ids[0]!), /referenced/)
  assert.throws(() => removeChannel(c, ids[2]!), /referenced/)
  assert.throws(() => addChannel(c, 'new'), /matrices/)
  assert.throws(() => renameChannel(c, ids[0]!, ids[1]!), /unique/)
  assert.deepEqual(c, before)
  renameChannel(c, ids[0]!, ' renamed ')
  assert.deepEqual(c.effects[0]!.channels, [' renamed '])
  const matrix = c.effects[1]!.effect
  if (matrix.kind !== 'channel-matrix') throw new Error('matrix required')
  assert.deepEqual(matrix.routes[0], { output: ' renamed ', inputs: [{ channel: ' renamed ', gain: 1 }] })
  assert.doesNotThrow(() => readChain(c))
  c.effects.splice(1, 1); removeChannel(c, ids[2]!); assert.deepEqual(c.effects[0]!.channels, [' renamed '])
})

test('millisecond delays require exact samples or an explicit quantization policy', () => {
  assert.equal(quantizeDelay(1, 48000, 'exact'), 48)
  assert.equal(quantizeDelay(1, 44100, 'exact'), null)
  assert.equal(quantizeDelay(1, 44100, 'nearest'), 44)
  assert.equal(quantizeDelay(1, 44100, 'floor'), 44)
  assert.equal(quantizeDelay(1, 44100, 'ceil'), 45)
  assert.equal(quantizeDelay(-1, 48000, 'nearest'), null)
  assert.equal(quantizeDelay(Infinity, 48000, 'nearest'), null)
  assert.equal(quantizeDelay(0.001, 48000, 'floor'), null, 'no hidden zero-sample delay stage')
})
