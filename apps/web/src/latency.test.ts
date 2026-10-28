import { test } from 'node:test'
import assert from 'node:assert/strict'
import { DelayDraft, readDelay } from './latency.ts'
import type { DelayRequest, DelayResult } from './latency.ts'
import { starter } from './dsp.ts'
import { copyDsp } from './dsp-json.ts'

function request(): DelayRequest { return { config: starter('empty'), probe: { blockFrames: 4, observationFrames: 16, referenceChannels: ['FL','FR'], amplitude: 1, detectionThreshold: 0, bypassed: false }, alignment: { channels: ['FL','FR'], feature: 'first-response' } } }
function response(req: DelayRequest): DelayResult { return { measurement: { mode: 'offline-impulse-response', sampleRateHz: 48000, probe: copyDsp(req.probe), responseWindowComplete: true, channels: [
  { channel: 'FL', firstResponseFrame: 3, strongestPeakFrame: 3, peakMagnitude: 1 },
  { channel: 'FR', firstResponseFrame: 7, strongestPeakFrame: 7, peakMagnitude: 1 },
] }, alignment: { mode: 'offline-feature-alignment', feature: 'first-response', sampleRateHz: 48000, targetFrame: 7, delays: [{ channel:'FL',frames:4 },{ channel:'FR',frames:0 }] }, alignmentError: null } }

test('delay data validates reference, full output identities and proposed compensation before edits', () => {
  const req = request(), valid = response(req)
  assert.deepEqual(readDelay(valid, req), valid)
  for (const mutate of [
    (r: DelayResult) => { r.measurement.probe.amplitude = 0.5 },
    (r: DelayResult) => { r.measurement.channels.reverse() },
    (r: DelayResult) => { r.measurement.channels[0]!.firstResponseFrame = 16 },
    (r: DelayResult) => { r.measurement.channels[0]!.peakMagnitude = Infinity },
    (r: DelayResult) => { r.alignment!.delays[0]!.frames = 5 },
    (r: DelayResult) => { r.alignment!.delays[0]!.channel = 'unknown' },
  ]) { const bad = copyDsp(valid); mutate(bad); assert.throws(() => readDelay(bad, req), /does not match/) }
})

test('silence is unknown delay and incomplete peak windows cannot authorize alignment', () => {
  const req = request(), r = response(req)
  r.measurement.channels[0]!.firstResponseFrame = null
  r.measurement.channels[0]!.strongestPeakFrame = null
  r.alignment = null; r.alignmentError = 'NoResponse'
  assert.equal(readDelay(r, req).measurement.channels[0]!.firstResponseFrame, null)
  const peak = request(); peak.alignment!.feature = 'strongest-peak'
  const partial = response(peak); partial.alignment!.feature = 'strongest-peak'; partial.measurement.responseWindowComplete = false
  assert.throws(() => readDelay(partial, peak), /does not match/)
})

test('delay measurements are explicit, exact snapshots and stale or failed work cannot publish a plan', async () => {
  const d = new DelayDraft(), req = request(), calls: string[] = []
  await d.measure({ post: async (path, body) => { calls.push(path); assert.deepEqual(body, req); return response(req) } }, req)
  assert.deepEqual(calls, ['/api/dsp/delay']); assert.ok(d.report?.alignment)
  let resolve!: (v: unknown) => void
  const pending = d.measure({ post: () => new Promise(r => { resolve = r }) }, req)
  d.changed(); resolve(response(req))
  assert.equal(await pending, false); assert.equal(d.report, null); assert.equal(d.request, null)
  await assert.rejects(d.measure({ post: async () => { throw new Error('offline') } }, req), /offline/)
  assert.equal(d.report, null)
})
