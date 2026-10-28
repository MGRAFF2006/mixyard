import { copyDsp } from './dsp-json.ts'
import { object, string } from './model.ts'
import type { Raw } from './model.ts'
import type { Api } from './api.ts'

export const filters = ['low-pass', 'high-pass', 'band-pass', 'notch', 'peaking', 'low-shelf', 'high-shelf', 'all-pass'] as const
export const effectKinds = ['gain', 'biquad', 'delay', 'compressor', 'expander', 'fir', 'channel-matrix'] as const
export type Filter = typeof filters[number]
export type EffectKind = typeof effectKinds[number]
export type Dynamics = Raw & { thresholdDb: number; ratio: number; kneeDb: number; attackMs: number; releaseMs: number; makeupDb: number; rangeDb: number }
export type Effect = (Raw & { kind: 'gain'; gain: number; invertPolarity: boolean }) |
  (Raw & { kind: 'biquad'; filter: Filter; frequencyHz: number; q: number; gainDb: number }) |
  (Raw & { kind: 'delay'; frames: number; wet: number; dry: number; feedback: number }) |
  (Raw & { kind: 'compressor' | 'expander'; parameters: Dynamics }) |
  (Raw & { kind: 'fir'; coefficients: number[] }) |
  (Raw & { kind: 'channel-matrix'; routes: MatrixRoute[] })
export type MatrixRoute = Raw & { output: string; inputs: (Raw & { channel: string; gain: number })[] }
export type EffectConfig = Raw & { name: string; enabled: boolean; channels?: string[] | null; effect: Effect }
export type ChainConfig = Raw & { name: string; format: Raw & { sampleRateHz: number; channels: string[] }; effects: EffectConfig[] }
export type Signal = { kind: 'impulse' | 'constant'; amplitude: number } | { kind: 'sine'; amplitude: number; frequencyHz: number } | { kind: 'samples'; samples: number[] }
export type PreviewRequest = { config: ChainConfig; frames: number; iterations: number; bypassed: boolean; signal: Signal }
export type Preview = {
  mode: 'offline-dsp-preview'; precision: 'float64'; frames: number; sampleRateHz: number; channels: string[]; output: number[]
  report: { frames: number; overFullScaleSamples: number; bypassed: boolean }
  meters: { peak: number; rms: number; overFullScaleSamples: number }[]
  admission: { memoryBytes: number; workUnitsPerBlock: number; algorithmicLatencyFrames: number; explicitDelayFrames: number; tail: { kind: 'finite'; frames: number } | { kind: 'infinite' } }
  timing: { measured: boolean; iterations: number; meanMs: number | null; maxMs: number | null; blockBudgetMs: number; deadlineMisses: number; overBudget: boolean; build: string; os: string; arch: string }
}
function fail(message: string): never { throw new Error(message) }
function list(v: unknown): unknown[] { return Array.isArray(v) ? v : fail('Expected a JSON array.') }
function finite(v: unknown): number { return typeof v === 'number' && Number.isFinite(v) ? v : fail('Expected a finite Float64 number.') }
function nonnegative(v: unknown): number { const n = finite(v); return n >= 0 ? n : fail('Expected a nonnegative number.') }
function integer(v: unknown, min = 0, max = Number.MAX_SAFE_INTEGER): number {
  const n = finite(v)
  return Number.isSafeInteger(n) && n >= min && n <= max ? n : fail(`Expected an integer in ${min}–${max}.`)
}
function bool(v: unknown): boolean { return typeof v === 'boolean' ? v : fail('Expected true or false.') }
function label(v: unknown): string { const s = string(v); return s.trim() ? s : fail('Names and channel IDs must not be blank.') }
function finiteTree(v: unknown): void {
  if (typeof v === 'number') finite(v)
  else if (v && typeof v === 'object') Object.values(v).forEach(finiteTree)
}
export function parseDspJson(text: string): unknown {
  if (new TextEncoder().encode(text).length > 1_048_576) fail('DSP JSON exceeds the web preview 1 MiB input budget. Use the offline renderer for larger jobs; no coefficients were truncated.')
  const v: unknown = JSON.parse(text); finiteTree(v); return v
}
export function readNumbers(text: string): number[] { return list(parseDspJson(text)).map(finite) }
// Shape checks only. Rust validates domains, coefficients, stability and resource limits.
// Retain original fields and opaque identities; never rebuild imports from editor defaults.
export function readChain(value: unknown): ChainConfig {
  finiteTree(value)
  const c = object(value), f = object(c.format)
  label(c.name); integer(f.sampleRateHz, 1, 0xffff_ffff)
  const channels = list(f.channels).map(label)
  if (!channels.length || new Set(channels).size !== channels.length) fail('Channel IDs must be nonempty and unique, in explicit order.')
  for (const value of list(c.effects)) {
    const e = object(value), p = object(e.effect)
    label(e.name); bool(e.enabled)
    if (p.kind === 'channel-matrix') {
      if (e.channels != null) fail('Channel matrices use explicit routes; remove their channels selector.')
    } else if (e.channels != null) {
      const selected = list(e.channels).map(label)
      if (!selected.length || new Set(selected).size !== selected.length || selected.some(id => !channels.includes(id))) fail(`Effect "${e.name}" needs nonempty, unique, known channel IDs. Correct its selection or format IDs.`)
    }
    switch (p.kind) {
      case 'channel-matrix': {
        const outputs: string[] = []
        for (const value of list(p.routes)) {
          const route = object(value), output = label(route.output)
          outputs.push(output)
          const inputs = list(route.inputs).map(value => { const input = object(value); finite(input.gain); return label(input.channel) })
          if (new Set(inputs).size !== inputs.length || inputs.some(id => !channels.includes(id))) fail('Matrix inputs must be unique, known channel IDs. Correct the route before importing.')
        }
        if (outputs.length !== channels.length || new Set(outputs).size !== outputs.length || channels.some(id => !outputs.includes(id))) fail('Matrix must include every current output ID exactly once. Correct routes before changing the format.')
        break
      }
      case 'gain': finite(p.gain); bool(p.invertPolarity); break
      case 'biquad':
        if (!filters.includes(p.filter as Filter)) fail('Unsupported biquad filter.')
        for (const k of ['frequencyHz', 'q', 'gainDb']) finite(p[k])
        break
      case 'delay': integer(p.frames, 1); for (const k of ['wet', 'dry', 'feedback']) finite(p[k]); break
      case 'compressor': case 'expander': {
        const d = object(p.parameters)
        for (const k of ['thresholdDb', 'ratio', 'kneeDb', 'attackMs', 'releaseMs', 'makeupDb', 'rangeDb']) finite(d[k])
        break
      }
      case 'fir': if (!list(p.coefficients).length) fail('FIR coefficients must not be empty.'); list(p.coefficients).forEach(finite); break
      default: fail('Unsupported effect kind.')
    }
  }
  return c as ChainConfig
}
export function newEffect(kind: EffectKind, channels: string[] = ['FL', 'FR']): EffectConfig {
  let effect: Effect
  switch (kind) {
    case 'gain': effect = { kind, gain: 1, invertPolarity: false }; break
    case 'biquad': effect = { kind, filter: 'peaking', frequencyHz: 1000, q: Math.SQRT1_2, gainDb: 0 }; break
    case 'delay': effect = { kind, frames: 960, wet: 0.2, dry: 0.8, feedback: 0 }; break
    case 'compressor': case 'expander': effect = { kind, parameters: { thresholdDb: kind === 'compressor' ? -18 : -40, ratio: 3, kneeDb: 6, attackMs: 5, releaseMs: 100, makeupDb: 0, rangeDb: 80 } }; break
    case 'fir': effect = { kind, coefficients: [1] }; break
    case 'channel-matrix': effect = { kind, routes: channels.map(id => ({ output: id, inputs: [{ channel: id, gain: 1 }] })) }; break
  }
  return { name: kind, enabled: true, effect }
}
export function starter(preset: 'empty' | 'trim' | 'echo' | 'fir'): ChainConfig {
  const chain: ChainConfig = { name: 'New chain', format: { sampleRateHz: 48000, channels: ['FL', 'FR'] }, effects: [] }
  if (preset === 'trim') { const e = newEffect('gain'); if (e.effect.kind === 'gain') e.effect.gain = 0.5; chain.effects = [e]; chain.name = 'Half-amplitude trim' }
  if (preset === 'echo') { chain.effects = [newEffect('delay')]; chain.name = '20 ms single echo at 48 kHz' }
  if (preset === 'fir') { const e = newEffect('fir'); if (e.effect.kind === 'fir') e.effect.coefficients = [1, 0, -0.1]; chain.effects = [e]; chain.name = 'Three-tap FIR example' }
  return chain
}
export function move<T>(items: T[], index: number, offset: -1 | 1) {
  const to = index + offset
  if (index < 0 || index >= items.length || to < 0 || to >= items.length) return
  const item = items.splice(index, 1)[0]!
  items.splice(to, 0, item)
}
export function readAdmission(value: unknown): Preview['admission'] {
  const a = object(value), tail = object(a.tail)
  for (const key of ['memoryBytes', 'workUnitsPerBlock', 'algorithmicLatencyFrames', 'explicitDelayFrames']) integer(a[key])
  if (tail.kind === 'finite') integer(tail.frames)
  else if (tail.kind !== 'infinite') fail('Unsupported tail metadata.')
  return a as Preview['admission']
}
export function readPreview(value: unknown, request: PreviewRequest): Preview {
  const v = object(value), r = object(v.report), a = object(v.admission), t = object(v.timing)
  if (v.mode !== 'offline-dsp-preview' || v.precision !== 'float64') fail('Unsupported DSP preview response.')
  if (v.frames !== request.frames || v.sampleRateHz !== request.config.format.sampleRateHz || JSON.stringify(v.channels) !== JSON.stringify(request.config.format.channels) || r.frames !== request.frames || r.bypassed !== request.bypassed) fail('Preview does not match the requested block and format.')
  if (list(v.output).length !== request.frames * request.config.format.channels.length) fail('Preview output is incomplete.')
  list(v.output).forEach(finite); integer(r.overFullScaleSamples, 0, request.frames * request.config.format.channels.length)
  if (list(v.meters).length !== request.config.format.channels.length) fail('Preview meters do not match the channels.')
  for (const value of list(v.meters)) { const m = object(value); nonnegative(m.peak); nonnegative(m.rms); integer(m.overFullScaleSamples, 0, request.frames) }
  readAdmission(a)
  bool(t.measured); integer(t.iterations, 1); integer(t.deadlineMisses, 0, request.iterations); bool(t.overBudget); nonnegative(t.blockBudgetMs)
  if (t.blockBudgetMs === 0 || (!t.measured && (t.overBudget || t.deadlineMisses !== 0))) fail('Inconsistent timing measurement status.')
  for (const k of ['meanMs', 'maxMs']) { if (t.measured) nonnegative(t[k]); else if (t[k] !== null) fail('Unmeasured timing must have null durations.') }
  for (const k of ['build', 'os', 'arch']) string(t[k])
  if (t.iterations !== request.iterations) fail('Timing repetitions do not match the request.')
  return v as Preview
}
export class DspDraft {
  config = starter('empty')
  bypassed = false
  report: Preview | null = null
  revision = 0
  changed() { this.revision++; this.report = null }
  async render(api: Pick<Api, 'post'>, request: PreviewRequest): Promise<boolean> {
    this.report = null
    const revision = this.revision, snapshot = copyDsp(request)
    const result = readPreview(await api.post('/api/dsp/preview', snapshot), snapshot)
    if (revision !== this.revision) return false
    this.report = result; return true
  }
  async import(api: Pick<Api, 'post'>, text: string): Promise<boolean> {
    const config = copyDsp(readChain(parseDspJson(text))), revision = this.revision
    const result = object(await api.post('/api/dsp/validate', { config, frames: 1 }))
    if (result.valid !== true || result.precision !== 'float64') fail('Core did not validate this Float64 chain.')
    readAdmission(result.admission)
    if (revision !== this.revision) return false
    this.config = config; this.changed(); return true
  }
}
