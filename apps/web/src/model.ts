// Shape checks protect the UI; routing policy is always validated by Rust.
// Keep the original objects, including fields this UI does not edit.
export type Raw = Record<string, unknown>
export type Level = Raw & { gain: number; mute?: boolean }
export type Selector = Raw & { kind: 'mic' | 'app' | 'call-return'; deviceUid?: string; applicationId?: string; group?: string }
export type Source = Raw & { id: string; selector: Selector }
export type Mix = Raw & { id: string; name: string; channels: number; sampleRateHz?: number; master?: Level; excludeGroups?: string[] }
export type Send = Level & { source: string; mix: string }
export type Output = Level & { id: string; mix: string; target: Raw & { kind: string; deviceUid?: string; endpointId?: string }; allowMicMonitor?: boolean }
export type Config = Raw & { schemaVersion: 2; revision: number; sources: Source[]; mixes: Mix[]; sends: Send[]; outputs: Output[]; mediaControls?: Raw[] }
export type Inventory = Raw & { schemaVersion: 2; revision: number; platform: string; evidence: 'simulated' | 'native-discovery'; sources: Raw[]; outputs: Raw[] }
export type Profile = { id: string; name: string; config: Config }
export type Snapshot = { format: string; schemaVersion: 1; storeRevision: number; configRevision: number; profiles: Profile[]; draft: Config | null; previousDraft: Config | null }
export type Status = { connected: boolean; snapshot: Raw | null; error: string | null }
export type Inspection = Raw & { planAvailable: boolean; preflightError: string | null; inventoryEvidence: string; findings: { subject: string; code: string }[]; sources: Raw[]; outputs: Raw[] }
export const MAX_INPUT_BYTES = 131_072
export const copy = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T
export const signature = (value: unknown) => JSON.stringify(value)
export function object(value: unknown): Raw {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) throw new Error('Expected a JSON object.')
  return value as Raw
}
export function array(value: unknown): unknown[] {
  if (!Array.isArray(value)) throw new Error('Expected a JSON array.')
  return value
}
export function string(value: unknown): string {
  if (typeof value !== 'string' || !value.trim()) throw new Error('Expected a nonempty string.')
  return value
}
export function integer(value: unknown): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) throw new Error('Expected an exact nonnegative integer.')
  return value
}
function bool(value: unknown) { if (typeof value !== 'boolean') throw new Error('Expected true or false.') }
function choice(value: unknown, choices: readonly string[]) { if (!choices.includes(string(value))) throw new Error(`Expected ${choices.join(', ')}.`) }
function optional(v: Raw, key: string, check: (value: unknown) => unknown) { if (v[key] !== undefined) check(v[key]) }
function level(value: unknown) {
  const v = object(value)
  if (typeof v.gain !== 'number' || !Number.isFinite(v.gain) || v.gain < 0 || v.gain > 1) throw new Error('Gain must be in 0–1.')
  optional(v, 'mute', bool)
}
function selector(value: unknown) {
  const v = object(value)
  choice(v.kind, ['mic', 'app', 'call-return'])
  string(v.kind === 'mic' ? v.deviceUid : v.applicationId)
  if (v.kind === 'call-return') string(v.group)
}
function target(value: unknown) {
  const v = object(value)
  choice(v.kind, ['device', 'virtual-input'])
  string(v.kind === 'device' ? v.deviceUid : v.endpointId)
}
function unique(items: unknown[], key: (v: Raw) => string) {
  const seen = new Set<string>()
  for (const item of items) { const id = key(object(item)); if (seen.has(id)) throw new Error(`Duplicate identity: ${id}`); seen.add(id) }
}
function schema(value: unknown) {
  const v = object(value)
  if (v.schemaVersion !== 2) throw new Error('Import schema-2 JSON; discovery envelopes are not inventories.')
  integer(v.revision)
  return v
}
export function readConfig(value: unknown): Config {
  const c = schema(value)
  const sources = array(c.sources), mixes = array(c.mixes), sends = array(c.sends), outputs = array(c.outputs)
  for (const group of [sources, mixes, outputs]) unique(group, v => string(v.id))
  unique(sends, v => JSON.stringify([string(v.source), string(v.mix)]))
  for (const item of sources) selector(object(item).selector)
  for (const item of mixes) {
    const m = object(item); string(m.name); integer(m.channels)
    optional(m, 'sampleRateHz', integer); optional(m, 'master', level)
    optional(m, 'excludeGroups', v => array(v).forEach(string))
  }
  sends.forEach(level)
  for (const item of outputs) { const o = object(item); string(o.mix); target(o.target); level(o); optional(o, 'allowMicMonitor', bool) }
  optional(c, 'mediaControls', v => array(v).forEach(item => {
    const p = object(item), policy = object(p.policy); string(p.deviceUid)
    choice(policy.mode, ['os-default', 'primary', 'all-routed-players'])
    if (policy.mode === 'primary') { string(policy.applicationId); if (policy.sessionId != null) string(policy.sessionId) }
  }))
  return c as Config
}
export function readInventory(value: unknown): Inventory {
  const v = schema(value)
  choice(v.platform, ['linux', 'windows', 'macos']); choice(v.evidence, ['simulated', 'native-discovery'])
  const format = (value: unknown) => { if (value != null) { const f = object(value); integer(f.channels); integer(f.sampleRateHz) } }
  for (const item of array(v.sources)) { const s = object(item); string(s.observedId); selector(s.selector); choice(s.origin, ['external', 'mixyard-output', 'mixyard-producer']); format(s.format) }
  for (const item of array(v.outputs)) { const o = object(item); string(o.observedId); target(o.target); format(o.format) }
  optional(v, 'limits', value => { for (const limit of Object.values(object(value))) if (limit !== null) integer(limit) })
  optional(v, 'mediaSessions', value => array(value).forEach(item => {
    const s = object(item); string(s.sessionId); string(s.applicationId)
    optional(s, 'playbackState', value => choice(value, ['playing', 'paused', 'stopped', 'unknown']))
    if (s.supportedActions != null) array(s.supportedActions).forEach(value => choice(value, ['play', 'pause', 'play-pause', 'next', 'previous']))
  }))
  return v as Inventory
}
export function parseImport(text: string): unknown {
  if (new TextEncoder().encode(text).length > MAX_INPUT_BYTES) throw new Error('Import exceeds 128 KiB.')
  const value: unknown = JSON.parse(text)
  // Refuse lossy JSON numeric imports instead of silently changing revisions/IDs.
  const exact = (v: unknown): void => {
    if (typeof v === 'number' && (!Number.isFinite(v) || (Number.isInteger(v) && !Number.isSafeInteger(v)))) throw new Error('JSON contains a number outside the exact supported range.')
    if (v && typeof v === 'object') Object.values(v).forEach(exact)
  }
  exact(value)
  return value
}
export function readSnapshot(value: unknown): Snapshot {
  const v = object(value)
  if (v.format !== 'mixyard-local-drafts' || v.schemaVersion !== 1) throw new Error('Unsupported profile store response.')
  integer(v.storeRevision); integer(v.configRevision)
  const profiles = array(v.profiles); unique(profiles, p => string(p.id))
  profiles.forEach(item => { const p = object(item); string(p.name); readConfig(p.config) })
  for (const key of ['draft', 'previousDraft']) if (v[key] !== null) readConfig(v[key])
  return v as Snapshot
}
export function readStatus(value: unknown): Status {
  const v = object(value); bool(v.connected)
  if (v.snapshot !== null) object(v.snapshot)
  if (v.error !== null && typeof v.error !== 'string') throw new Error('Unsupported runtime status.')
  return v as Status
}
export function readInspection(value: unknown): Inspection {
  const v = object(value)
  if (v.schemaVersion !== 1 || v.mode !== 'routing-inspection' || v.liveLevels !== null || v.nativeState !== 'unknown') throw new Error('Unsupported inspection response.')
  bool(v.planAvailable); integer(v.configRevision); integer(v.inventoryRevision)
  choice(v.inventoryEvidence, ['simulated', 'native-discovery'])
  if (v.preflightError !== null) string(v.preflightError)
  if (v.planAvailable !== (v.plan !== null) || v.planAvailable !== (v.preflightError === null)) throw new Error('Inconsistent inspection response.')
  if (v.plan !== null) {
    const plan = object(v.plan)
    if (plan.mode !== 'dry-run' || plan.schemaVersion !== 2 || plan.configRevision !== v.configRevision || plan.inventoryRevision !== v.inventoryRevision) throw new Error('Inspection plan does not match inputs.')
  }
  array(v.findings).forEach(item => { const f = object(item); string(f.subject); string(f.code) })
  for (const key of ['sources', 'mixes', 'sends', 'outputs', 'capacity']) array(v[key]).forEach(object)
  return v as Inspection
}
export function setLevel(level: Level, patch: Partial<Pick<Level, 'gain' | 'mute'>>) {
  const next = { ...level, ...patch }; // Validate before touching the draft.
  if (typeof next.gain !== 'number' || !Number.isFinite(next.gain) || next.gain < 0 || next.gain > 1) throw new Error('Gain must be in 0–1.')
  if (next.mute !== undefined) bool(next.mute)
  Object.assign(level, patch)
}
