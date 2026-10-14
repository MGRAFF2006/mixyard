// This boundary checks shapes and evidence labels, never routing policy.
export const MAX_INPUT_BYTES = 131_072
const MAX_RESPONSE_BYTES = 2_097_152
export type SnapshotInput = { configJson: string; inventoryJson: string }
const reasons = [
  'preflight-failed', 'config-validation-failed', 'inventory-schema-unsupported', 'capacity-exceeded', 'capacity-unknown',
  'source-missing', 'source-ambiguous', 'output-missing', 'output-ambiguous', 'unsupported-observed-format',
  'format-unknown', 'conflicting-observation', 'recapture-forbidden', 'call-return-alias-forbidden',
  'excluded-call-return', 'mic-monitor-consent-required', 'send-not-configured', 'send-muted', 'send-zero',
  'mix-master-muted', 'mix-master-zero', 'output-muted', 'output-zero', 'unknown-source-reference',
  'unknown-mix-reference', 'permission-unknown', 'consumer-unknown', 'native-state-unknown',
] as const
type Reason = typeof reasons[number]
type ObjectValue = Record<string, unknown>
function fail(): never { throw new Error('Unsupported inspection response. Previous results were cleared.') }
function object(value: unknown): ObjectValue {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return fail()
  return value as ObjectValue
}
function array<T>(value: unknown, read: (item: unknown) => T): T[] {
  if (!Array.isArray(value)) return fail()
  return value.map(read)
}
function string(value: unknown): string { return typeof value === 'string' ? value : fail() }
function number(value: unknown): number { return typeof value === 'number' && Number.isFinite(value) ? value : fail() }
function integer(value: unknown): number { return Number.isSafeInteger(value) && number(value) >= 0 ? number(value) : fail() }
function boolean(value: unknown): boolean { return typeof value === 'boolean' ? value : fail() }
function literal<T extends string>(value: unknown, choices: readonly T[]): T {
  return choices.find(choice => choice === value) ?? fail()
}
const reason = (value: unknown) => literal(value, reasons)
const resolution = (value: unknown) => value === null ? null : literal(value, ['selected', 'missing', 'ambiguous', 'unsupported-format'] as const)
function unknown(value: unknown) { return literal(value, ['unknown'] as const) }
function level(value: unknown) {
  const v = object(value)
  return { gain: number(v.gain), mute: boolean(v.mute) }
}
function selector(value: unknown) {
  const v = object(value)
  const kind = literal(v.kind, ['mic', 'app', 'call-return'] as const)
  return kind === 'mic' ? { kind, deviceUid: string(v.deviceUid) }
    : { kind, applicationId: string(v.applicationId), ...(kind === 'call-return' ? { group: string(v.group) } : {}) }
}
function target(value: unknown) {
  const v = object(value)
  const kind = literal(v.kind, ['device', 'virtual-input'] as const)
  return kind === 'device' ? { kind, deviceUid: string(v.deviceUid) } : { kind, endpointId: string(v.endpointId) }
}
function mix(value: unknown) {
  const v = object(value)
  return { id: string(v.id), name: string(v.name), channels: integer(v.channels), sampleRateHz: integer(v.sampleRateHz), master: level(v.master), excludeGroups: array(v.excludeGroups, string) }
}
function send(value: unknown) {
  const v = object(value)
  return { source: string(v.source), mix: string(v.mix), ...level(v) }
}
function format(value: unknown) {
  if (value === null) return null
  const v = object(value)
  return { channels: integer(v.channels), sampleRateHz: integer(v.sampleRateHz) }
}
function source(value: unknown) {
  const v = object(value), configured = object(v.configured)
  return {
    configured: { id: string(configured.id), selector: selector(configured.selector) },
    candidates: array(v.candidates, item => {
      const c = object(item)
      return { observedId: string(c.observedId), selector: selector(c.selector), origin: literal(c.origin, ['external', 'mixyard-output', 'mixyard-producer'] as const), format: format(c.format) }
    }),
    plannedResolution: resolution(v.plannedResolution), permission: unknown(v.permission), captureState: unknown(v.captureState), reasons: array(v.reasons, reason),
  }
}
function output(value: unknown) {
  const v = object(value), c = object(v.configured)
  return {
    configured: { id: string(c.id), mix: string(c.mix), target: target(c.target), ...level(c), allowMicMonitor: boolean(c.allowMicMonitor) },
    mix: v.mix === null ? null : mix(v.mix),
    candidates: array(v.candidates, item => {
      const c = object(item)
      return { observedId: string(c.observedId), target: target(c.target), format: format(c.format) }
    }),
    plannedResolution: resolution(v.plannedResolution), consumer: unknown(v.consumer), nativeState: unknown(v.nativeState), reasons: array(v.reasons, reason),
    paths: array(v.paths, item => {
      const p = object(item)
      return {
        source: string(p.source), configuredSend: p.configuredSend === null ? null : send(p.configuredSend),
        intendedPotentialSharing: boolean(p.intendedPotentialSharing), excluded: boolean(p.excluded),
        plannedEffectiveCoefficient: p.plannedEffectiveCoefficient === null ? null : number(p.plannedEffectiveCoefficient),
        appliedSharing: unknown(p.appliedSharing), reasons: array(p.reasons, reason),
      }
    }),
  }
}

export function validateInput(input: SnapshotInput): SnapshotInput {
  for (const [label, text] of [['Config', input.configJson], ['Inventory', input.inventoryJson]]) {
    if (new TextEncoder().encode(text).length > MAX_INPUT_BYTES) throw new Error(`${label} exceeds the 128 KiB import limit.`)
    let value: ObjectValue
    try { value = object(JSON.parse(text)) }
    catch { throw new Error(`${label} must contain a JSON object.`) }
    if (value.schemaVersion !== 2) throw new Error(`${label} must use schema 2. Discovery envelope 1 is not an inventory.`)
    if (!Number.isSafeInteger(value.revision) || number(value.revision) < 0) throw new Error(`${label} revision must be an exact integer in 0–9007199254740991.`)
  }
  return input
}

export function readInspection(value: unknown) {
  if (typeof value !== 'string' || new TextEncoder().encode(value).length > MAX_RESPONSE_BYTES) return fail()
  const envelope = object(JSON.parse(value)), r = object(envelope.report)
  if (envelope.schemaVersion !== 1 || envelope.provenance !== 'user-import' || envelope.freshness !== 'unverified'
    || r.schemaVersion !== 1 || r.mode !== 'routing-inspection' || r.submixes !== 'not-implemented' || r.liveLevels !== null) return fail()
  const report = {
    configRevision: integer(r.configRevision), inventoryRevision: integer(r.inventoryRevision),
    inventoryEvidence: literal(r.inventoryEvidence, ['simulated', 'native-discovery'] as const),
    platform: literal(r.platform, ['linux', 'windows', 'macos'] as const),
    nativeState: unknown(r.nativeState), liveLevels: null, playerPauseState: unknown(r.playerPauseState), captureState: unknown(r.captureState),
    planAvailable: boolean(r.planAvailable), preflightError: r.preflightError === null ? null : string(r.preflightError),
    findings: array(r.findings, item => { const f = object(item); return { subject: string(f.subject), code: reason(f.code) } }),
    capacity: array(r.capacity, item => { const c = object(item); return { resource: string(c.resource), requested: integer(c.requested), maximum: c.maximum === null ? null : integer(c.maximum), reason: c.reason === null ? null : reason(c.reason) } }),
    sources: array(r.sources, source), mixes: array(r.mixes, mix), sends: array(r.sends, send), outputs: array(r.outputs, output),
  }
  let plan = null
  if (r.plan !== null) {
    const p = object(r.plan)
    if (p.schemaVersion !== 2 || p.mode !== 'dry-run' || p.configRevision !== report.configRevision
      || p.inventoryRevision !== report.inventoryRevision || p.inventoryEvidence !== report.inventoryEvidence || p.platform !== report.platform) return fail()
    plan = {
      mixes: array(p.mixes, item => { const m = object(item); return { id: string(m.id), peakBound: number(m.peakBound) } }),
      outputs: array(p.outputs, item => { const o = object(item); return { id: string(o.id), peakBound: number(o.peakBound) } }),
      diagnostics: array(p.diagnostics, item => { const d = object(item); return { subject: string(d.subject), code: string(d.code), message: string(d.message) } }),
    }
  }
  if (report.planAvailable !== (plan !== null) || report.planAvailable !== (report.preflightError === null)
    || report.outputs.some(o => o.paths.some(p => report.planAvailable ? p.plannedEffectiveCoefficient === null : p.plannedEffectiveCoefficient !== null))) return fail()
  return { ...report, plan, raw: JSON.stringify(envelope, null, 2) }
}
export type Inspection = ReturnType<typeof readInspection>

// One outstanding request. Input changes invalidate even a successful late reply.
export class InspectionSession {
  result: Inspection | null = null
  error = ''
  busy = false
  private generation = 0
  private alive = true
  invalidate() { this.generation++; this.result = null; this.error = '' }
  dispose() { this.invalidate(); this.alive = false }
  async run(input: SnapshotInput, invoke: (input: SnapshotInput) => Promise<Inspection>) {
    this.invalidate()
    if (this.busy) { this.error = 'An inspection is already running. Wait, then inspect the current inputs.'; return }
    this.busy = true
    const generation = this.generation
    try {
      const result = await invoke(validateInput(input))
      if (this.alive && generation === this.generation) this.result = result
    } catch (error) {
      if (this.alive && generation === this.generation) this.error = error instanceof Error ? error.message : String(error)
    } finally { this.busy = false }
  }
}

export const reasonText: Record<Reason, string> = {
  'preflight-failed': 'Core preflight rejected this snapshot; see the detail below.',
  'config-validation-failed': 'Configuration failed core validation, including headroom or policy constraints.',
  'inventory-schema-unsupported': 'Unsupported inventory schema.',
  'capacity-exceeded': 'Requested routes exceed the reported backend capacity.',
  'capacity-unknown': 'Backend capacity is unknown; native preflight is still required.',
  'source-missing': 'No exact source observation; no fallback.', 'source-ambiguous': 'Multiple exact source candidates; none selected.',
  'output-missing': 'No exact output observation; no fallback.', 'output-ambiguous': 'Multiple exact output candidates; none selected.',
  'unsupported-observed-format': 'Observed channel count or sample rate is unsupported.', 'format-unknown': 'Observed format is unknown.',
  'conflicting-observation': 'Conflicting observations share an identity.', 'recapture-forbidden': 'Mixyard output/producer recapture is forbidden.',
  'call-return-alias-forbidden': 'An app selector aliases a call return.', 'excluded-call-return': 'Call-return group excluded by mix policy.',
  'mic-monitor-consent-required': 'Physical mic monitoring lacks configured consent, even at mute/zero.',
  'send-not-configured': 'No send configured.', 'send-muted': 'Send is muted.', 'send-zero': 'Send gain is zero.',
  'mix-master-muted': 'Mix master is muted.', 'mix-master-zero': 'Mix master gain is zero.', 'output-muted': 'Output is muted.', 'output-zero': 'Output gain is zero.',
  'unknown-source-reference': 'Send references an unknown source.', 'unknown-mix-reference': 'Route references an unknown mix.',
  'permission-unknown': 'Permission is unknown.', 'consumer-unknown': 'Consumer selection/attachment is unknown.', 'native-state-unknown': 'Applied native audio state is unknown.',
}
