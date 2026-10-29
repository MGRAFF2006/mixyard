// Portable codec intent is independent of routing Config and audio grants.
export type Encoding = { mode: 'raw' } | { mode: 'zstd'; level: number } | { mode: 'opus'; bitrate: number; complexity: number; vbr: boolean; conversion: 'normalized-f32-clamp-v1' }
export type CodecConfig = { version: 1; streamId: string; generation: string; encoding: Encoding; allowLossy: boolean }
export const MAX_CODEC_BYTES = 131_072
export const MAX_CODEC_DRAFT_BYTES = 8 * 1024 * 1024
const U64_MAX = 18446744073709551615n
const clone = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T
export const codecSignature = (value: unknown) => JSON.stringify(value, (_key, item: unknown) => {
  if (item && typeof item === 'object' && !Array.isArray(item)) return Object.fromEntries(Object.entries(item).sort(([a], [b]) => a.localeCompare(b)))
  return item
})
function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw Error('Expected an object.')
  return value as Record<string, unknown>
}
function exact(value: unknown, keys: readonly string[]) {
  const v = record(value)
  if (Object.keys(v).length !== keys.length || keys.some(k => !Object.hasOwn(v, k))) throw Error(`Expected exactly these fields: ${keys.join(', ')}.`)
  return v
}
function text(value: unknown): string { if (typeof value !== 'string') throw Error('Expected text.'); return value }
function nonempty(value: unknown): string { const v = text(value); if (!v.trim()) throw Error('Expected nonempty text.'); return v }
function bool(value: unknown): boolean { if (typeof value !== 'boolean') throw Error('Expected true or false.'); return value }
function integer(value: unknown, min = 0, max = Number.MAX_SAFE_INTEGER): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || Object.is(value, -0) || value < min || value > max) throw Error(`Expected an exact integer from ${min} to ${max}.`)
  return value
}
export function decimalU64(value: unknown): string {
  if (typeof value !== 'string' || !/^[1-9][0-9]{0,19}$/.test(value) || BigInt(value) > U64_MAX) throw Error('Use a canonical nonzero decimal-u64 string (1–18446744073709551615).')
  return value
}
function list(value: unknown): unknown[] { if (!Array.isArray(value)) throw Error('Expected an array.'); return value }
// Check lexical integers before JSON.parse can round fractions to integers.
// This bounded parser also rejects duplicate keys, including escaped aliases.
export function parseCodecJson(source: string, maxBytes = MAX_CODEC_BYTES): unknown {
  if (new TextEncoder().encode(source).length > maxBytes) throw Error(`Codec JSON exceeds ${maxBytes} bytes.`)
  let at = 0
  const whitespace = () => { while (/[\x20\t\r\n]/.test(source[at] ?? '') && at < source.length) at++ }
  function stringToken(): string {
    const start = at++
    while (at < source.length && source[at] !== '"') { if (source[at] === '\\') at++; at++ }
    if (at >= source.length) throw Error('Unterminated JSON string.')
    return JSON.parse(source.slice(start, ++at)) as string
  }
  function visit(depth: number): unknown {
    if (depth > 24) throw Error('Codec JSON nesting exceeds 24 levels.')
    whitespace()
    const c = source[at]
    if (c === '"') return stringToken()
    if (c === '{' || c === '[') {
      const object = c === '{', close = object ? '}' : ']'
      const result: Record<string, unknown> = Object.create(null), values: unknown[] = [], seen = new Set<string>()
      at++; whitespace()
      if (source[at] === close) { at++; return object ? result : values }
      while (at < source.length) {
        if (object) {
          whitespace(); if (source[at] !== '"') throw Error('Expected a JSON field name.')
          const key = stringToken()
          if (seen.has(key)) throw Error(`Duplicate JSON field: ${key}.`)
          seen.add(key); whitespace(); if (source[at++] !== ':') throw Error('Expected a colon.')
          result[key] = visit(depth + 1)
        } else values.push(visit(depth + 1))
        whitespace()
        if (source[at] === close) { at++; return object ? result : values }
        if (source[at++] !== ',') throw Error('Expected a comma.')
      }
      throw Error('Unterminated JSON collection.')
    }
    for (const [token, value] of [['true', true], ['false', false], ['null', null]] as const) {
      if (source.startsWith(token, at)) { at += token.length; return value }
    }
    const token = source.slice(at).match(/^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?/)?.[0]
    if (!token || !/^(0|[1-9][0-9]*)$/.test(token)) throw Error('Use exact nonnegative integer literals in codec JSON.')
    at += token.length; return integer(Number(token))
  }
  const result = visit(0); whitespace()
  if (at !== source.length) throw Error('Unexpected trailing JSON data.')
  return result
}
export function readCodecConfig(value: unknown): CodecConfig {
  const v = exact(value, ['version', 'streamId', 'generation', 'encoding', 'allowLossy'])
  if (v.version !== 1) throw Error('Unsupported codec config version; expected 1.')
  decimalU64(v.streamId); decimalU64(v.generation); bool(v.allowLossy)
  const e = record(v.encoding)
  if (e.mode === 'raw') exact(e, ['mode'])
  else if (e.mode === 'zstd') { exact(e, ['mode', 'level']); integer(e.level, 1, 3) }
  else if (e.mode === 'opus') {
    exact(e, ['mode', 'bitrate', 'complexity', 'vbr', 'conversion'])
    integer(e.bitrate, 6000, 512000); integer(e.complexity, 0, 10); bool(e.vbr)
    if (e.conversion !== 'normalized-f32-clamp-v1') throw Error('Opus requires normalized-f32-clamp-v1 conversion.')
    if (v.allowLossy !== true) throw Error('Opus requires explicit lossy consent.')
  } else throw Error('Choose raw, zstd or opus; no automatic fallback is supported.')
  return clone(v) as CodecConfig
}
export function parseCodecConfig(source: string): CodecConfig { return readCodecConfig(parseCodecJson(source)) }
export type CodecForm = { streamId: string; generation: string; mode: string; level: string; bitrate: string; complexity: string; vbr: boolean; allowLossy: boolean }
export function newCodecForm(): CodecForm { return { streamId: '', generation: '', mode: 'raw', level: '1', bitrate: '96000', complexity: '5', vbr: true, allowLossy: false } }
function formInteger(source: string, label: string): number {
  if (!/^(0|[1-9][0-9]*)$/.test(source)) throw Error(`${label}: enter an exact nonnegative integer, without decimals or exponents.`)
  return integer(Number(source))
}
export function configFromForm(form: CodecForm): CodecConfig {
  let encoding: unknown = { mode: form.mode }
  if (form.mode === 'zstd') encoding = { mode: 'zstd', level: formInteger(form.level, 'Zstd level') }
  if (form.mode === 'opus') encoding = { mode: 'opus', bitrate: formInteger(form.bitrate, 'Opus bitrate'), complexity: formInteger(form.complexity, 'Opus complexity'), vbr: form.vbr, conversion: 'normalized-f32-clamp-v1' }
  return readCodecConfig({ version: 1, streamId: form.streamId, generation: form.generation, encoding, allowLossy: form.allowLossy })
}
export function formFromConfig(config: CodecConfig): CodecForm {
  const f = { ...newCodecForm(), streamId: config.streamId, generation: config.generation, mode: config.encoding.mode, allowLossy: config.allowLossy }
  if (config.encoding.mode === 'zstd') f.level = String(config.encoding.level)
  if (config.encoding.mode === 'opus') { f.bitrate = String(config.encoding.bitrate); f.complexity = String(config.encoding.complexity); f.vbr = config.encoding.vbr }
  return f
}
export type CodecBinding = { targetId: string; deviceId: string; authorityId: string; revision: number; epoch: number }
export type OriginalSpec = { stream: string; name: string; codec: 'raw-s16-le' | 'raw-s24-le' | 'raw-s32-le' | 'raw-f32-le' | 'raw-f64-le'; sample_rate: number; channels: string[]; frames_per_packet: number; provenance: { sources: { peer: string; source: string; groups: string[] }[]; visited_peers: string[] } }
export type CodecConstraints = { maxSourceChannels: number; zstdLevels: number[]; zstdMaxWindowBytes: number; zstdDictionaries: boolean; opusRates: number[]; opusMonoPreservesSourceChannel: boolean; opusStereoOrderedChannels: string[]; opusFrameMicros: number[]; opusMinBitrate: number; opusMaxBitrate: number; opusMaxComplexity: number; opusFloatInput: string; automaticResampling: false; automaticChannelMapping: false }
export type CodecMeasurements = { blocks: number; rawBytes: number; envelopeBytes: number; fragmentBytes: number; wallNanos: number; cpuNanos: number | null; cpuUnavailableReason: string | null; lookaheadFrames: number; contextBytes: number }
// Matches network/src/codec_transport.rs CodecCapability (camelCase serde).
export type CodecCapability = {
  version: 1; request: CodecConfig; available: boolean; agreementState: 'local-capability-only' | 'send-agreement-confirmed' | 'receive-admitted'; availabilityScope: string; unavailableReason: string | null; constraints: CodecConstraints; zstdVersion: number | null; opusVersion: string | null;
  maxRawBytes: number; maxEncodedBytes: number; maxEnvelopeBytes: number | null; maxFragmentsPerBlock: number | null; reservedWindowBytes: number | null; maxAggregateReassemblyBytes: number; maxWorkWallNanos: number; lookaheadFrames: number | null; contextBytes: number | null; measurements: CodecMeasurements | null; measurementUnavailableReason: string | null; networkDelayNanos: number | null; physicalDelayNanos: number | null; delayUnavailableReason: string
}
export type CodecTimeline = { version: 1; request: CodecConfig; sourceSpec: OriginalSpec; nextSourceSampleFrame: string; bitPreserving: boolean; destinationConversion: 'normalized-f32-clamp-v1' | null; codecLookaheadFrames: number }
export type CodecEvidence = { timeline: CodecTimeline | null; binding: CodecBinding; evidenceKind: 'authenticated-session' | 'fixture'; direction: 'send' | 'receive'; originalSpec: OriginalSpec; capability: CodecCapability }
// Host owns authentication, exact Spec/grants, active TLS and service reads.
// There is deliberately no apply/write operation or endpoint in this contract.
export interface CodecProbe { probe(binding: Readonly<CodecBinding>, request: Readonly<CodecConfig>, signal: AbortSignal): Promise<unknown> }
function readBinding(value: unknown): CodecBinding {
  const v = exact(value, ['targetId', 'deviceId', 'authorityId', 'revision', 'epoch'])
  for (const key of ['targetId', 'deviceId', 'authorityId']) nonempty(v[key])
  integer(v.revision); integer(v.epoch); return v as CodecBinding
}
function stringList(value: unknown): string[] { return list(value).map(nonempty) }
function readSpec(value: unknown): OriginalSpec {
  const v = exact(value, ['stream', 'name', 'codec', 'sample_rate', 'channels', 'frames_per_packet', 'provenance'])
  decimalU64(v.stream); nonempty(v.name)
  if (!['raw-s16-le', 'raw-s24-le', 'raw-s32-le', 'raw-f32-le', 'raw-f64-le'].includes(text(v.codec))) throw Error('Expected the original raw PCM format.')
  integer(v.sample_rate, 1, 0xffffffff); integer(v.frames_per_packet, 1, 65535)
  const channels = stringList(v.channels)
  if (!channels.length || channels.length > 128 || new Set(channels).size !== channels.length) throw Error('Expected unique ordered source channels.')
  const p = exact(v.provenance, ['sources', 'visited_peers']), sources = list(p.sources), peers = stringList(p.visited_peers), identities = new Set<string>()
  if (!sources.length || !peers.length || new Set(peers).size !== peers.length) throw Error('Incomplete or repeated source provenance.')
  for (const source of sources) { const s = exact(source, ['peer', 'source', 'groups']); const id = codecSignature([nonempty(s.peer), nonempty(s.source)]); stringList(s.groups); if (identities.has(id)) throw Error('Duplicate source provenance.'); identities.add(id) }
  return v as OriginalSpec
}
const capabilityKeys = ['version', 'request', 'available', 'agreementState', 'availabilityScope', 'unavailableReason', 'constraints', 'zstdVersion', 'opusVersion', 'maxRawBytes', 'maxEncodedBytes', 'maxEnvelopeBytes', 'maxFragmentsPerBlock', 'reservedWindowBytes', 'maxAggregateReassemblyBytes', 'maxWorkWallNanos', 'lookaheadFrames', 'contextBytes', 'measurements', 'measurementUnavailableReason', 'networkDelayNanos', 'physicalDelayNanos', 'delayUnavailableReason']
export function readCodecEvidence(value: unknown, binding: CodecBinding, request: CodecConfig): CodecEvidence {
  const e = exact(typeof value === 'string' ? parseCodecJson(value) : value, ['binding', 'evidenceKind', 'direction', 'originalSpec', 'capability', 'timeline'])
  const actual = readBinding(e.binding)
  if (!sameBinding(actual, binding)) throw Error('Capability belongs to another device, authority, revision or session.')
  if (!['authenticated-session', 'fixture'].includes(text(e.evidenceKind)) || !['send', 'receive'].includes(text(e.direction))) throw Error('Unsupported capability qualification or direction.')
  const spec = readSpec(e.originalSpec), c = exact(e.capability, capabilityKeys), r = readCodecConfig(c.request)
  if (spec.stream !== request.streamId || codecSignature(r) !== codecSignature(readCodecConfig(request))) throw Error('Capability does not match this exact stream, generation and encoding request.')
  if (c.version !== 1) throw Error('Unsupported capability version.')
  bool(c.available); nonempty(c.availabilityScope); nonempty(c.delayUnavailableReason)
  if (!['local-capability-only', 'send-agreement-confirmed', 'receive-admitted'].includes(text(c.agreementState))) throw Error('Unsupported agreement state.')
  if ((c.agreementState === 'send-agreement-confirmed' && e.direction !== 'send') || (c.agreementState === 'receive-admitted' && e.direction !== 'receive')) throw Error('Agreement direction mismatch.')
  for (const key of ['unavailableReason', 'opusVersion', 'measurementUnavailableReason']) if (c[key] !== null) nonempty(c[key])
  if (!c.available && !c.unavailableReason) throw Error('Unavailable capability requires a reason.')
  for (const key of ['maxRawBytes', 'maxEncodedBytes', 'maxAggregateReassemblyBytes', 'maxWorkWallNanos']) integer(c[key])
  for (const key of ['zstdVersion', 'maxEnvelopeBytes', 'maxFragmentsPerBlock', 'reservedWindowBytes', 'lookaheadFrames', 'contextBytes', 'networkDelayNanos', 'physicalDelayNanos']) if (c[key] !== null) integer(c[key])
  const constraints = exact(c.constraints, ['maxSourceChannels', 'zstdLevels', 'zstdMaxWindowBytes', 'zstdDictionaries', 'opusRates', 'opusMonoPreservesSourceChannel', 'opusStereoOrderedChannels', 'opusFrameMicros', 'opusMinBitrate', 'opusMaxBitrate', 'opusMaxComplexity', 'opusFloatInput', 'automaticResampling', 'automaticChannelMapping'])
  for (const key of ['maxSourceChannels', 'zstdMaxWindowBytes', 'opusMinBitrate', 'opusMaxBitrate', 'opusMaxComplexity']) integer(constraints[key])
  for (const key of ['zstdLevels', 'opusRates', 'opusFrameMicros']) list(constraints[key]).forEach(v => integer(v))
  stringList(constraints.opusStereoOrderedChannels); nonempty(constraints.opusFloatInput); bool(constraints.zstdDictionaries); bool(constraints.opusMonoPreservesSourceChannel)
  if (constraints.automaticResampling !== false || constraints.automaticChannelMapping !== false) throw Error('Automatic format conversion is unsupported.')
  if (c.measurements === null) nonempty(c.measurementUnavailableReason)
  else {
    const m = exact(c.measurements, ['blocks', 'rawBytes', 'envelopeBytes', 'fragmentBytes', 'wallNanos', 'cpuNanos', 'cpuUnavailableReason', 'lookaheadFrames', 'contextBytes'])
    for (const key of ['blocks', 'rawBytes', 'envelopeBytes', 'fragmentBytes', 'wallNanos', 'lookaheadFrames', 'contextBytes']) integer(m[key])
    if (m.cpuNanos === null) nonempty(m.cpuUnavailableReason)
    else integer(m.cpuNanos)
  }
  if (e.timeline !== null) {
    const t = exact(e.timeline, ['version', 'request', 'sourceSpec', 'nextSourceSampleFrame', 'bitPreserving', 'destinationConversion', 'codecLookaheadFrames'])
    if (t.version !== 1 || codecSignature(readCodecConfig(t.request)) !== codecSignature(r) || codecSignature(readSpec(t.sourceSpec)) !== codecSignature(spec)) throw Error('Timeline does not match the exact request and original Spec.')
    if (t.nextSourceSampleFrame !== '0') decimalU64(t.nextSourceSampleFrame)
    integer(t.codecLookaheadFrames)
    if (t.bitPreserving !== (r.encoding.mode !== 'opus') || t.destinationConversion !== (r.encoding.mode === 'opus' ? 'normalized-f32-clamp-v1' : null)) throw Error('Timeline bit preservation or conversion contradicts the exact request.')
    if (c.lookaheadFrames !== null && c.lookaheadFrames !== t.codecLookaheadFrames) throw Error('Timeline lookahead contradicts capability.')
  }
  return clone(e) as CodecEvidence
}
export function sameBinding(a: CodecBinding | null, b: CodecBinding | null): boolean {
  return a === null || b === null ? a === b : a.targetId === b.targetId && a.deviceId === b.deviceId && a.authorityId === b.authorityId && a.revision === b.revision && a.epoch === b.epoch
}
export type CodecDraft = { id: number; form: CodecForm; owner: CodecBinding | null }
export class CodecWorkspace {
  drafts: CodecDraft[] = [{ id: 1, form: newCodecForm(), owner: null }]
  selectedId = 1
  importText = ''
  error = ''
  notice = ''
  evidence: CodecEvidence | null = null
  busy = false
  binding: CodecBinding | null = null
  active = true
  private operation = 0
  private nextId = 2
  private controller: AbortController | null = null
  get selected(): CodecDraft { return this.drafts.find(d => d.id === this.selectedId)! }
  get dirty(): boolean { return this.drafts.length !== 1 || codecSignature(this.drafts[0]!.form) !== codecSignature(newCodecForm()) || !!this.importText }
  get validation(): { config: CodecConfig | null; error: string } {
    try { return { config: configFromForm(this.selected.form), error: '' } } catch (error) { return { config: null, error: error instanceof Error ? error.message : String(error) } }
  }
  invalidate() { this.operation++; this.controller?.abort(); this.controller = null; this.busy = false; this.evidence = null; this.notice = '' }
  setBinding(binding: CodecBinding | null) { if (!sameBinding(this.binding, binding)) { this.invalidate(); this.binding = binding ? clone(readBinding(binding)) : null } }
  setActive(active: boolean) { if (this.active !== active) { this.active = active; this.invalidate() } }
  changed() { this.invalidate(); this.error = '' }
  select(id: number) { if (this.drafts.some(d => d.id === id)) { this.invalidate(); this.selectedId = id; this.error = '' } }
  add(form = newCodecForm()) { if (this.drafts.length >= 64) throw Error('Keep at most 64 stream drafts in this workspace. Export drafts before removing one.'); this.invalidate(); const draft = { id: this.nextId++, form: clone(form), owner: this.binding ? clone(this.binding) : null }; this.drafts.push(draft); this.selectedId = draft.id; return draft }
  remove() { this.invalidate(); this.drafts = this.drafts.filter(d => d.id !== this.selectedId); if (!this.drafts.length) this.drafts.push({ id: this.nextId++, form: newCodecForm(), owner: null }); this.selectedId = this.drafts[0]!.id }
  bind() { this.invalidate(); this.selected.owner = this.binding ? clone(this.binding) : null; this.notice = this.binding ? 'Draft bound to this exact management session for capability reads only.' : 'No authenticated management identity is available.' }
  importIntent() { const config = parseCodecConfig(this.importText); const draft = this.add(formFromConfig(config)); draft.owner = null; this.notice = 'Portable intent imported into a new draft. No authority or audio grant was imported.' }
  backup(): string { return JSON.stringify({ format: 'mixyard-codec-drafts', version: 1, forms: this.drafts.map(d => d.form), importText: this.importText }, null, 2) + '\n' }
  restore(source: string) {
    const v = exact(parseCodecJson(source, MAX_CODEC_DRAFT_BYTES), ['format', 'version', 'forms', 'importText'])
    if (v.format !== 'mixyard-codec-drafts' || v.version !== 1) throw Error('Expected a codec drafts backup version 1.')
    const forms = list(v.forms)
    if (!forms.length || forms.length + this.drafts.length > 64) throw Error('Restore must keep the combined workspace within 64 drafts.')
    const checked = forms.map(f => {
      const r = exact(f, ['streamId', 'generation', 'mode', 'level', 'bitrate', 'complexity', 'vbr', 'allowLossy'])
      for (const key of ['streamId', 'generation', 'mode', 'level', 'bitrate', 'complexity']) text(r[key])
      bool(r.vbr); bool(r.allowLossy); return clone(r) as CodecForm
    })
    const raw = text(v.importText)
    if (this.importText && raw && this.importText !== source) throw Error('Export or clear the current import text before restoring another import buffer.')
    for (const form of checked) this.add(form).owner = null
    this.importText = raw; this.notice = 'Drafts restored without authority or capability evidence. Existing stream drafts kept.'
  }
  async probe(host?: CodecProbe) {
    const config = this.validation.config, binding = this.binding
    if (!host || !config || !binding || !this.active || !sameBinding(this.selected.owner, binding) || this.busy) return
    this.invalidate(); const operation = this.operation, controller = new AbortController(), captured = clone(config), owner = clone(binding)
    this.controller = controller; this.busy = true; this.error = ''
    try {
      const response = await host.probe(clone(owner), clone(captured), controller.signal)
      if (this.operation !== operation || !this.active || !sameBinding(this.binding, owner) || codecSignature(this.validation.config) !== codecSignature(captured)) return
      this.evidence = readCodecEvidence(response, owner, captured)
    } catch (error) { if (this.operation === operation && !controller.signal.aborted) this.error = error instanceof Error ? error.message : String(error) }
    finally { if (this.operation === operation) { this.busy = false; this.controller = null } }
  }
}
