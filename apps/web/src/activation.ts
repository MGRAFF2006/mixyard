import { array, integer, object, readConfig, readInventory, string } from './model.ts'
import type { Config, Inventory, Raw } from './model.ts'
import { dspJson } from './dsp-json.ts'
import { branches, parseAutomationJson, readAction, readCatalog, readEvaluation, readGraph, readLiveContext, readRequest, standardCatalog } from './situations.ts'
import type { Action, Definition, Evaluation, Graph, LiveContext, SituationRequest } from './situations.ts'

export const MAX_DURATION = 86_400_000
export type Declaration = Raw & { key: Raw & { name: 'device.hardware-monitor-declared' | 'manual.flag'; subject: string }; value: boolean }
export type Settings = Raw & {
  graph: Graph | null
  catalog: Definition[]
  declarations: Declaration[]
  transitionPolicy: Raw & { settleMs: number; minimumHoldMs: number; cooldownMs: number }
  observer: Raw & { applications: Record<string, string>; hostLabel: string | null; timezone: string }
}
export type SettingsSnapshot = Raw & { format: 'mixyard-situation-settings'; schemaVersion: 1; settingsRevision: number; settings: Settings }
export type Blocked = { code: string; message: string }
export type Manual = { mode: 'until-cleared' } | { mode: 'for'; untilMs: number }
export type ManualCommand = { mode: 'until-cleared' } | { mode: 'for'; durationMs: number } | { mode: 'clear' }
export type Status = Raw & {
  mode: 'stored-intent-only'; audioApplied: false; backendAvailable: false; armed: boolean
  manualOverride: Manual | null; settingsRevision: number; storeRevision: number; configRevision: number
  inventoryRevision: number | null; manualRevision: number; decision: Raw & { kind: string }
  blocked: Blocked | null; lastAppliedAtMs: number | null; review: { id: string; expiresAtMs: number } | null
}
export type SensitivePath = Raw & { source: string | null; selector: Raw | null; mix: string; output: string; target: Raw; reason: string }
export type Change = Raw & { identity: unknown; before: unknown; after: unknown }
export const CHANGE_GROUPS = ['sources', 'sends', 'mixes', 'masters', 'outputs', 'mediaControls'] as const
export type ProfilesPreview = Raw & {
  changes: Record<typeof CHANGE_GROUPS[number], Change[]>
  affectedSensitivePathsBefore: SensitivePath[]; affectedSensitivePathsAfter: SensitivePath[]; sensitivePaths: SensitivePath[]
  unavailableIdentities: { sources: Raw[]; outputs: Raw[] }; plan: Raw
}
export type Candidate = { candidate: Config; preview: ProfilesPreview }
export type Authority = { epoch: string; settingsRevision: number; storeRevision: number; configRevision: number; inventoryRevision: number; manualRevision: number }
export type Review = { id: string; expiresAtMs: number; authority: Authority; actions: (Candidate & { terminal: string; action: Action })[] }
export type Live = Raw & {
  mode: 'stored-intent-only'; audioApplied: false; backendAvailable: false; status: Status
  evaluation: Evaluation | null; request: SituationRequest | null; context: LiveContext | null
  preview: Candidate | null; inventory: Inventory | null; blocked: Blocked | null; review: Review | null
}
// Object property order is not part of JSON identity. Numbers still compare by
// Float64 bits, including -0; array order and every raw property remain material.
export function sameJson(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true
  if (!a || !b || typeof a !== 'object' || typeof b !== 'object' || Array.isArray(a) !== Array.isArray(b)) return false
  const left = Object.keys(a), right = Object.keys(b)
  return left.length === right.length && left.every(k => Object.hasOwn(b, k) && sameJson((a as Raw)[k], (b as Raw)[k]))
}
function bool(v: unknown): asserts v is boolean { if (typeof v !== 'boolean') throw Error('Expected true or false.') }
function choice(v: unknown, choices: readonly string[]) { if (!choices.includes(string(v))) throw Error(`Expected ${choices.join(', ')}.`) }
function list(v: unknown, max = 4096) { const a = array(v); if (a.length > max) throw Error(`Response exceeds ${max} items; no items were hidden.`); return a }
function nullable(v: unknown, read: (v: unknown) => unknown) { if (v !== null) read(v) }
function identity(v: unknown) { const s = string(v); if (new TextEncoder().encode(s).length > 1024) throw Error('Identity exceeds 1024 bytes.'); return s }
export function duration(v: unknown, minimum = 0) { const n = integer(v); if (n < minimum || n > MAX_DURATION) throw Error(`Duration must be ${minimum}–86400000 ms.`); return n }
export function defaultSettings(): Settings {
  return { graph: null, catalog: [], declarations: [], transitionPolicy: { settleMs: 1000, minimumHoldMs: 2000, cooldownMs: 2000 }, observer: { applications: {}, hostLabel: null, timezone: 'Europe/Berlin' } }
}
export function readSettings(v: unknown): Settings {
  const s = object(v), catalog = readCatalog(s.catalog)
  if (s.graph === null) { if (catalog.length) throw Error('A missing graph requires an empty catalog.') }
  else readGraph(s.graph, catalog)
  const keys = new Set<string>()
  list(s.declarations, 512).forEach(v => {
    const d = object(v), k = object(d.key)
    choice(k.name, ['device.hardware-monitor-declared', 'manual.flag']); identity(k.subject); bool(d.value)
    const key = dspJson([k.name, k.subject]); if (keys.has(key)) throw Error('Duplicate declaration.'); keys.add(key)
  })
  const policy = object(s.transitionPolicy)
  for (const k of ['settleMs', 'minimumHoldMs', 'cooldownMs']) duration(policy[k])
  const observer = object(s.observer), apps = Object.entries(object(observer.applications))
  if (apps.length > 32) throw Error('At most 32 executable mappings are supported.')
  apps.forEach(([path, app]) => {
    identity(path); identity(app)
    if (!path.startsWith('/') && !/^[A-Za-z]:[\\/]/.test(path) && !/^\\\\[^\\]+\\[^\\]+/.test(path)) throw Error('Use an exact absolute executable path on the service host.')
  })
  nullable(observer.hostLabel, identity); identity(observer.timezone)
  if (new TextEncoder().encode(dspJson(s)).length > 1_048_576) throw Error('Settings exceed 1 MiB.')
  return s as Settings
}
export function readSettingsSnapshot(v: unknown): SettingsSnapshot {
  const s = object(v)
  if (s.format !== 'mixyard-situation-settings' || s.schemaVersion !== 1) throw Error('Unsupported situation settings format.')
  integer(s.settingsRevision); readSettings(s.settings)
  return s as SettingsSnapshot
}
export function parseSettings(text: string): SettingsSnapshot {
  if (new TextEncoder().encode(text).length > 1_048_576) throw Error('Settings import exceeds 1 MiB.')
  return readSettingsSnapshot(parseAutomationJson(text))
}
function storedOnly(v: Raw) {
  if (v.mode !== 'stored-intent-only' || v.audioApplied !== false || v.backendAvailable !== false) throw Error('Unsupported audio authority. This console only supports stored intent with no native audio backend.')
}
function readBlocked(v: unknown): Blocked | null {
  if (v === null) return null
  const b = object(v); string(b.code); string(b.message); return b as Blocked
}
function reviewToken(v: unknown) { const r = object(v); string(r.id); integer(r.expiresAtMs); return r as { id: string; expiresAtMs: number } }
export const REVISION_KEYS = ['settingsRevision', 'storeRevision', 'configRevision', 'inventoryRevision', 'manualRevision'] as const
export function readActivationStatus(v: unknown): Status {
  const s = object(v); storedOnly(s); bool(s.armed)
  REVISION_KEYS.forEach(k => k === 'inventoryRevision' ? nullable(s[k], integer) : integer(s[k]))
  nullable(s.lastAppliedAtMs, integer); nullable(s.review, reviewToken); readBlocked(s.blocked)
  if (s.manualOverride !== null) {
    const m = object(s.manualOverride); choice(m.mode, ['until-cleared', 'for'])
    if (m.mode === 'for') integer(m.untilMs)
  }
  const d = object(s.decision)
  choice(d.kind, ['disarmed', 'armed', 'manual-override', 'no-change', 'already-applied', 'candidate', 'waiting', 'applied-stored-intent', 'write-failed'])
  if (d.kind === 'waiting') {
    // Clearing a manual pause may report waiting before the next capture.
    const fields = ['settleRemainingMs', 'holdRemainingMs', 'cooldownRemainingMs']
    if (fields.some(k => k in d)) fields.forEach(k => integer(d[k]))
  }
  if (d.kind === 'applied-stored-intent') {
    if (d.audioApplied !== false || integer(d.storeRevision) !== s.storeRevision || integer(d.configRevision) !== s.configRevision) throw Error('Applied decision does not match stored authority.')
  }
  return s as Status
}
function target(v: unknown) { const t = object(v); choice(t.kind, ['device', 'virtual-input']); string(t.kind === 'device' ? t.deviceUid : t.endpointId) }
function selector(v: unknown) { const s = object(v); choice(s.kind, ['mic', 'app', 'call-return']); string(s.kind === 'mic' ? s.deviceUid : s.applicationId); if (s.kind === 'call-return') string(s.group) }
function finite(v: unknown) { if (typeof v !== 'number' || !Number.isFinite(v)) throw Error('Expected finite Float64.'); return v }
function resolution(v: unknown, output: boolean) {
  const r = object(v); string(output ? r.id : r.source); choice(r.status, ['selected', 'missing', 'ambiguous', 'unsupported-format'])
  list(r.observedIds, 256).forEach(string)
  if (output) target(r.target)
  else { selector(r.selector); nullable(r.format, v => { const f = object(v); integer(f.channels); integer(f.sampleRateHz) }) }
  return r
}
function readPlan(v: unknown, candidate: Config, inventory: Inventory): Raw {
  const p = object(v)
  if (p.schemaVersion !== 2 || p.mode !== 'dry-run' || integer(p.configRevision) !== candidate.revision || integer(p.inventoryRevision) !== inventory.revision || p.inventoryEvidence !== inventory.evidence || p.platform !== inventory.platform) throw Error('Preview plan does not match candidate and inventory.')
  list(p.sources, 256).forEach(v => resolution(v, false))
  list(p.outputs, 256).forEach(v => {
    const o = resolution(v, true); string(o.mix); bool(o.mute); bool(o.allowMicMonitor)
    for (const k of ['gain', 'effectiveGain', 'peakBound']) finite(o[k])
    string(o.channelMapping); nullable(o.rateConversionRequired, bool)
  })
  list(p.mixes, 256).forEach(v => {
    const m = object(v); string(m.id); string(m.name); integer(m.channels); integer(m.sampleRateHz)
    const level = object(m.master); finite(level.gain); bool(level.mute); list(m.excludeGroups, 256).forEach(string); finite(m.peakBound)
  })
  list(p.sends, 256).forEach(v => {
    const s = object(v); string(s.source); string(s.mix); bool(s.mute); nullable(s.observedId, string)
    finite(s.gain); finite(s.effectiveGain); string(s.channelMapping); nullable(s.rateConversionRequired, bool)
  })
  list(p.diagnostics).forEach(v => { const d = object(v); string(d.subject); string(d.code); string(d.message) })
  const identities = (items: unknown[], keys: string[]) => items.map(v => dspJson(keys.map(k => object(v)[k]))).sort()
  for (const [key, planKeys, configKeys] of [
    ['sources', ['source'], ['id']], ['mixes', ['id'], ['id']],
    ['sends', ['source', 'mix'], ['source', 'mix']], ['outputs', ['id'], ['id']],
  ] as const) {
    if (dspJson(identities(array(p[key]), [...planKeys])) !== dspJson(identities(candidate[key], [...configKeys]))) throw Error('Preview plan omitted or changed candidate identities.')
  }
  // These are static identities/parameters copied by the planner, not computed
  // routing results. Bind them to the concrete candidate shown for approval.
  const requireFields = (row: Raw, expected: Raw) => {
    if (!Object.entries(expected).every(([key, value]) => sameJson(row[key], value))) throw Error('Preview plan contradicts candidate routing.')
  }
  for (const value of array(p.sources)) {
    const row = object(value), source = candidate.sources.find(s => s.id === row.source)!
    requireFields(row, { selector: source.selector })
  }
  for (const value of array(p.mixes)) {
    const row = object(value), mix = candidate.mixes.find(m => m.id === row.id)!
    requireFields(row, { name: mix.name, channels: mix.channels, sampleRateHz: mix.sampleRateHz ?? 48000,
      master: { gain: mix.master?.gain ?? 1, mute: mix.master?.mute ?? false } })
    if (!sameJson([...array(row.excludeGroups)].sort(), [...(mix.excludeGroups ?? [])].sort())) throw Error('Preview plan contradicts candidate exclusions.')
  }
  for (const value of array(p.sends)) {
    const row = object(value), send = candidate.sends.find(s => s.source === row.source && s.mix === row.mix)!
    requireFields(row, { gain: send.gain, mute: send.mute ?? false })
  }
  for (const value of array(p.outputs)) {
    const row = object(value), output = candidate.outputs.find(o => o.id === row.id)!
    requireFields(row, { mix: output.mix, target: output.target, gain: output.gain, mute: output.mute ?? false, allowMicMonitor: output.allowMicMonitor ?? false })
  }
  return p
}
function sensitive(v: unknown): SensitivePath {
  const p = object(v); nullable(p.source, string); nullable(p.selector, selector); string(p.mix); string(p.output); target(p.target)
  if ((p.source === null) !== (p.selector === null)) throw Error('Sensitive path source and selector must be paired.')
  choice(p.reason, ['potential-recording', 'microphone-monitoring']); return p as SensitivePath
}
function readCandidate(v: unknown, inventory: Inventory | null, currentRevision: number): Candidate {
  if (!inventory) throw Error('A concrete preview requires server inventory.')
  const c = object(v), candidate = readConfig(c.candidate), p = object(c.preview), changes = object(p.changes)
  if (candidate.revision <= currentRevision) throw Error('Candidate revision must advance captured config authority.')
  // A replacement can remove 256 old identities and add 256 different ones.
  CHANGE_GROUPS.forEach(k => list(changes[k], 512).forEach(v => {
    const c = object(v), identity = list(c.identity, 2); identity.forEach(string)
    if (identity.length !== (k === 'sends' ? 2 : 1)) throw Error('Malformed profile change identity.')
    if (!('before' in c) || !('after' in c) || (c.before === null && c.after === null)) throw Error('Malformed profile change.')
    nullable(c.before, object); nullable(c.after, object)
  }))
  for (const k of ['affectedSensitivePathsBefore', 'affectedSensitivePathsAfter', 'sensitivePaths']) list(p[k]).forEach(sensitive)
  const unavailable = object(p.unavailableIdentities)
  list(unavailable.sources, 256).forEach(v => resolution(v, false)); list(unavailable.outputs, 256).forEach(v => resolution(v, true))
  readPlan(p.plan, candidate, inventory)
  const plan = object(p.plan)
  for (const k of ['sources', 'outputs']) {
    if (!sameJson(unavailable[k], array(plan[k]).filter(v => object(v).status !== 'selected'))) throw Error('Unavailable identities differ from the complete preview plan.')
  }
  return c as Candidate
}
function normalizedAction(v: Action) {
  return v.kind === 'no-change' ? { kind: v.kind } : { kind: v.kind, profileId: v.profileId ?? null, overrides: v.overrides.map(o => ({ scope: o.scope, source: o.source ?? null, mix: o.mix ?? null, output: o.output ?? null, mute: o.mute ?? null, gain: o.gain ?? null })) }
}
export function reachableSelects(graph: Graph) {
  const visited = new Set<string>(), pending = [graph.root], result: { terminal: string; action: Action }[] = []
  while (pending.length) {
    const id = pending.pop()!; if (visited.has(id)) continue; visited.add(id)
    const node = graph.nodes.find(n => n.id === id)!
    if (node.kind === 'decision') branches.forEach(([key]) => pending.push(node[key]))
    else if (node.action.kind === 'select') result.push({ terminal: id, action: node.action })
  }
  return result
}
export function readLive(v: unknown, saved: SettingsSnapshot, wantsReview: boolean): Live {
  const l = object(v); storedOnly(l)
  const status = readActivationStatus(l.status), inventory = l.inventory === null ? null : readInventory(l.inventory)
  const blocked = readBlocked(l.blocked)
  if (status.settingsRevision !== saved.settingsRevision || !sameJson(status.blocked, blocked)) throw Error('Live status does not match saved settings or blocked state. Read saved settings again.')
  const request = l.request === null ? null : readRequest(l.request)
  if (request) {
    if (request.manualOverride !== false || !sameJson(request.graph, saved.settings.graph) || !sameJson(request.catalog, saved.settings.catalog)) throw Error('Server request differs from saved policy or claims manual authority.')
    for (const k of ['storeRevision', 'configRevision', 'manualRevision'] as const) if (request.context[k] !== status[k]) throw Error('Server request revisions differ from status.')
    if (status.inventoryRevision !== null && request.context.inventoryRevision !== status.inventoryRevision) throw Error('Server inventory authority differs from request.')
  }
  if (inventory && inventory.revision !== status.inventoryRevision) throw Error('Live inventory revision differs from status.')
  if ((l.evaluation === null) !== (request === null)) throw Error('Live evaluation and request must be paired.')
  // The adapter selects the existing shape validator, never the user-facing label.
  const evaluation = request ? readEvaluation({ mode: 'simulation', evaluation: l.evaluation }, request) : null
  let context: LiveContext | null = null
  if (l.context !== null) {
    const catalog = [...standardCatalog().filter(d => !request?.catalog.some(c => c.name === d.name)), ...(request?.catalog ?? [])]
    context = readLiveContext({ ...object(l.context), mode: 'read-only-live-context', catalog })
    if (request && (context.nowMs !== request.nowMs || !sameJson({ ...context.snapshot, facts: context.snapshot.facts.filter(f => request.catalog.some(d => d.name === f.key.name)) }, request.snapshot))) throw Error('Live capture does not match the evaluated request.')
  }
  if (request && !context) throw Error('Server evaluation is missing its capture.')
  const preview = l.preview === null ? null : readCandidate(l.preview, inventory, status.configRevision)
  let prospectiveRevision = preview?.candidate.revision ?? null
  let review: Review | null = null
  if (l.review !== null && l.review !== undefined) {
    if (!wantsReview || blocked || !inventory || !request) throw Error('Review cannot grant authority without a complete unblocked capture.')
    const r = object(l.review), authority = object(r.authority); reviewToken(r); string(authority.epoch)
    REVISION_KEYS.forEach(k => { if (integer(authority[k]) !== status[k]) throw Error('Review authority differs from status.') })
    if (authority.epoch !== request.snapshot.epoch || !status.review || status.review.id !== r.id || status.review.expiresAtMs !== r.expiresAtMs || integer(r.expiresAtMs) < request.nowMs) throw Error('Review token differs from captured authority.')
    const expected = reachableSelects(request.graph), actions = list(r.actions, 256), seen = new Set<string>()
    actions.forEach(v => {
      const a = object(v), id = string(a.terminal), action = readAction(a.action), terminal = expected.find(e => e.terminal === id)
      if (!terminal || seen.has(id) || dspJson(normalizedAction(action)) !== dspJson(normalizedAction(terminal.action))) throw Error('Review action differs from reachable Select terminals.')
      seen.add(id)
      const parsed = readCandidate(a, inventory, status.configRevision)
      if (prospectiveRevision !== null && prospectiveRevision !== parsed.candidate.revision) throw Error('Review candidates disagree on prospective config revision.')
      prospectiveRevision = parsed.candidate.revision
    })
    if (seen.size !== expected.length) throw Error('Review omitted reachable Select actions; no actions were hidden.')
    review = r as Review
  }
  return { ...l, status, inventory, blocked, request, evaluation, context, preview, review } as Live
}
export function sameAuthority(a: Status | Authority, b: Status | Authority) { return REVISION_KEYS.every(k => a[k] === b[k]) }
