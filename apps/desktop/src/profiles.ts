// Shape/evidence validation only. Store validation and comparison belong to Rust.
export const MAX_INVENTORY_BYTES = 131_072
const MAX_REPLY_BYTES = 2_097_152
export type Json = null | boolean | number | string | Json[] | { [key: string]: Json }
type ObjectValue = { [key: string]: Json }
function fail(): never { throw new Error('Unsupported profile response. Refresh and select again.') }
function object(v: Json | undefined): ObjectValue { return v && typeof v === 'object' && !Array.isArray(v) ? v : fail() }
function string(v: Json | undefined): string { return typeof v === 'string' ? v : fail() }
function revision(v: Json | undefined): number { return typeof v === 'number' && Number.isSafeInteger(v) && v >= 0 ? v : fail() }
function array(v: Json | undefined): Json[] { return Array.isArray(v) ? v : fail() }
function finite(v: Json): void {
  if (typeof v === 'number' && !Number.isFinite(v)) fail()
  if (v && typeof v === 'object') for (const [key, value] of Object.entries(v)) {
    if (key === 'revision' || key.endsWith('Revision')) { if (value !== null) revision(value) }
    finite(value)
  }
}
function config(v: Json | undefined) {
  const c = object(v)
  if (c.schemaVersion !== 2) fail()
  revision(c.revision)
  for (const key of ['sources', 'sends', 'mixes', 'outputs', 'mediaControls']) array(c[key]).forEach(object)
  return { ...c, schemaVersion: 2 as const, revision: revision(c.revision) }
}
function envelope(value: unknown, command: 'list' | 'preview') {
  if (typeof value !== 'string' || new TextEncoder().encode(value).length > MAX_REPLY_BYTES) fail()
  const e = object(JSON.parse(value) as Json)
  finite(e)
  if (e.schemaVersion !== 1 || e.mode !== 'local-profile-preview' || e.command !== command
    || e.provenance !== 'current-user-local-store' || e.freshness !== 'read-snapshot'
    || e.audioApplied !== false || e.confirmedWorkingSetup !== null || e.storedConsentIsFreshNativeConsent !== false) fail()
  return e
}
export function readProfiles(value: unknown) {
  const e = envelope(value, 'list'), ids = new Set<string>()
  if (e.storeState !== 'present' && e.storeState !== 'missing') fail()
  const profiles = array(e.profiles).map(item => {
    const p = object(item), id = string(p.id), name = string(p.name)
    if (!id.trim() || !name.trim() || ids.has(id)) fail()
    ids.add(id)
    return { id, name, config: config(p.config) }
  })
  const draft = e.draft === null ? null : config(e.draft)
  const storeRevision = revision(e.storeRevision)
  if (e.storeState === 'missing' && (profiles.length || draft || storeRevision !== 0)) fail()
  return { storeRevision, storeState: e.storeState, profiles, draft }
}
export type ProfileList = ReturnType<typeof readProfiles>
export type PreviewRequest = { command: 'preview'; storeRevision: number; profileId: string; inventoryJson: string }
export function previewRequest(storeRevision: number, profileId: string, inventoryJson: string): PreviewRequest {
  revision(storeRevision)
  if (!profileId) throw new Error('Select a stable profile ID after Refresh.')
  if (new TextEncoder().encode(inventoryJson).length > MAX_INVENTORY_BYTES) throw new Error('Inventory exceeds the 128 KiB import limit.')
  const inventory = object(JSON.parse(inventoryJson) as Json)
  if (inventory.schemaVersion !== 2) throw new Error('Import schema-2 inventory; discovery envelope 1 is not an inventory.')
  revision(inventory.revision)
  return { command: 'preview', storeRevision, profileId, inventoryJson }
}
export const changeKinds = ['sources', 'sends', 'mixes', 'masters', 'outputs', 'mediaControls'] as const
export function readProfilePreview(value: unknown, request: PreviewRequest) {
  const e = envelope(value, 'preview'), p = object(e.preview), changes = object(p.changes), plan = object(p.plan)
  const inventory = object(JSON.parse(request.inventoryJson) as Json)
  if (e.storeRevision !== request.storeRevision || e.profileId !== request.profileId || e.inventoryRevision !== inventory.revision
    || plan.schemaVersion !== 2 || plan.mode !== 'dry-run' || plan.inventoryRevision !== e.inventoryRevision
    || !['simulated', 'native-discovery'].includes(string(plan.inventoryEvidence)) || !['linux', 'windows', 'macos'].includes(string(plan.platform))) fail()
  revision(plan.configRevision)
  if (e.draftRevision !== null) revision(e.draftRevision)
  const parsedChanges = Object.fromEntries(changeKinds.map(kind => [kind, array(changes[kind]).map(item => {
    const c = object(item)
    array(c.identity)
    if (c.before !== null) object(c.before)
    if (c.after !== null) object(c.after)
    return { identity: c.identity, before: c.before!, after: c.after! }
  })])) as Record<typeof changeKinds[number], { identity: Json; before: Json; after: Json }[]>
  for (const key of ['affectedSensitivePathsBefore', 'affectedSensitivePathsAfter', 'sensitivePaths']) array(p[key]).forEach(object)
  const unavailable = object(p.unavailableIdentities)
  array(unavailable.sources).forEach(object); array(unavailable.outputs).forEach(object)
  for (const key of ['sources', 'outputs', 'mixes', 'diagnostics']) array(plan[key]).forEach(object)
  return {
    storeRevision: revision(e.storeRevision), profileId: string(e.profileId), inventoryRevision: revision(e.inventoryRevision),
    draftRevision: e.draftRevision === null ? null : revision(e.draftRevision), changes: parsedChanges,
    affectedSensitivePathsBefore: array(p.affectedSensitivePathsBefore), affectedSensitivePathsAfter: array(p.affectedSensitivePathsAfter),
    sensitivePaths: array(p.sensitivePaths), unavailableIdentities: unavailable,
    plan: { ...plan, platform: string(plan.platform), inventoryEvidence: string(plan.inventoryEvidence) },
    raw: JSON.stringify(e, null, 2),
  }
}
export type ProfilePreview = ReturnType<typeof readProfilePreview>

export class ProfileSession {
  snapshot: ProfileList | null = null
  selectedId = ''
  result: ProfilePreview | null = null
  error = ''
  busy = false
  private generation = 0
  private alive = true
  invalidate() { this.generation++; this.result = null; this.error = '' }
  inputChanged() { this.invalidate(); this.selectedId = '' }
  select(id: string) { this.invalidate(); this.selectedId = this.snapshot?.profiles.some(p => p.id === id) ? id : '' }
  dispose() { this.inputChanged(); this.alive = false }
  private async run(work: () => Promise<void>) {
    if (this.busy) { this.inputChanged(); this.snapshot = null; this.error = 'A profile request is already running. Wait, then Refresh.'; return }
    this.busy = true
    try { await work() } finally { this.busy = false }
  }
  async refresh(invoke: () => Promise<ProfileList>) {
    this.inputChanged(); this.snapshot = null
    const generation = this.generation
    await this.run(async () => {
      try {
        const snapshot = await invoke()
        if (this.alive && generation === this.generation) this.snapshot = snapshot
      } catch (error) { if (this.alive && generation === this.generation) this.reject(error) }
    })
  }
  async preview(inventoryJson: string, invoke: (r: PreviewRequest) => Promise<ProfilePreview>) {
    this.invalidate()
    const generation = this.generation
    await this.run(async () => {
      try {
        if (!this.snapshot) throw new Error('Refresh the local store first.')
        const request = previewRequest(this.snapshot.storeRevision, this.selectedId, inventoryJson)
        const result = await invoke(request)
        if (this.alive && generation === this.generation) this.result = result
      } catch (error) { if (this.alive && generation === this.generation) this.reject(error) }
    })
  }
  reject(error: unknown) {
    this.inputChanged(); this.snapshot = null
    this.error = error instanceof Error ? error.message : String(error)
  }
}
