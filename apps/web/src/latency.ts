import type { Api } from './api.ts'
import type { ChainConfig } from './dsp.ts'
import { copyDsp } from './dsp-json.ts'
import { object } from './model.ts'

export type Feature = 'first-response' | 'strongest-peak'
export type DelayRequest = { config: ChainConfig; probe: { blockFrames: number; observationFrames: number; referenceChannels: string[]; amplitude: number; detectionThreshold: number; bypassed: boolean }; alignment: { channels: string[]; feature: Feature } | null }
export type Plan = { mode: 'offline-feature-alignment'; feature: Feature; sampleRateHz: number; targetFrame: number; delays: { channel: string; frames: number }[] }
export type DelayResult = { measurement: { mode: 'offline-impulse-response'; sampleRateHz: number; probe: DelayRequest['probe']; responseWindowComplete: boolean; channels: { channel: string; firstResponseFrame: number | null; strongestPeakFrame: number | null; peakMagnitude: number }[] }; alignment: Plan | null; alignmentError: string | null }
function fail(): never { throw new Error('Delay response is incomplete or does not match this chain/probe. No alignment was applied.') }
function array(v: unknown): unknown[] { return Array.isArray(v) ? v : fail() }
function frame(v: unknown, max: number): number { return typeof v === 'number' && Number.isSafeInteger(v) && v >= 0 && v <= max ? v : fail() }
function equalIds(a: unknown, b: string[]) { if (JSON.stringify(a) !== JSON.stringify(b)) fail() }
export function readDelay(value: unknown, request: DelayRequest): DelayResult {
  const result = object(value), m = object(result.measurement), p = object(m.probe)
  if (m.mode !== 'offline-impulse-response' || m.sampleRateHz !== request.config.format.sampleRateHz || typeof m.responseWindowComplete !== 'boolean') fail()
  for (const key of ['blockFrames', 'observationFrames', 'amplitude', 'detectionThreshold', 'bypassed'] as const) if (!Object.is(p[key], request.probe[key])) fail()
  equalIds(p.referenceChannels, request.probe.referenceChannels)
  const channels = array(m.channels)
  if (channels.length !== request.config.format.channels.length) fail()
  for (const [index, value] of channels.entries()) {
    const c = object(value)
    if (c.channel !== request.config.format.channels[index] || typeof c.peakMagnitude !== 'number' || !Number.isFinite(c.peakMagnitude) || c.peakMagnitude < 0) fail()
    for (const key of ['firstResponseFrame', 'strongestPeakFrame']) if (c[key] !== null) frame(c[key], request.probe.observationFrames - 1)
    if ((c.firstResponseFrame === null) !== (c.strongestPeakFrame === null)) fail()
  }
  if (result.alignmentError !== null && typeof result.alignmentError !== 'string') fail()
  if (result.alignment !== null) {
    const a = object(result.alignment)
    if (!request.alignment || a.mode !== 'offline-feature-alignment' || a.feature !== request.alignment.feature || a.sampleRateHz !== m.sampleRateHz || result.alignmentError !== null) fail()
    frame(a.targetFrame, request.probe.observationFrames - 1)
    const delays = array(a.delays)
    equalIds(delays.map(d => object(d).channel), request.alignment.channels)
    for (const value of delays) {
      const d = object(value); frame(d.frames, 65_536)
      const c = channels.map(object).find(c => c.channel === d.channel) ?? fail()
      const position = a.feature === 'first-response' ? c.firstResponseFrame : c.strongestPeakFrame
      if (position === null || (position as number) + (d.frames as number) !== a.targetFrame) fail()
    }
    if (a.feature === 'strongest-peak' && !m.responseWindowComplete) fail()
  }
  return result as DelayResult
}

export class DelayDraft {
  report: DelayResult | null = null
  revision = 0
  request: DelayRequest | null = null
  changed() { this.revision++; this.report = null; this.request = null }
  async measure(api: Pick<Api, 'post'>, request: DelayRequest): Promise<boolean> {
    this.report = null; this.request = null
    const revision = this.revision, snapshot = copyDsp(request)
    const response = readDelay(await api.post('/api/dsp/delay', snapshot), snapshot)
    if (revision !== this.revision) return false
    this.request = snapshot; this.report = response; return true
  }
}
