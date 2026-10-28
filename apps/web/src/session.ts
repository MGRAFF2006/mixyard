import { Api, ApiError } from './api.ts'
import { copy, object, readConfig, readInspection, readSnapshot, readStatus, signature } from './model.ts'
import type { Config, Inspection, Inventory, Snapshot, Status } from './model.ts'

export type Edit = { kind: 'draft'; config: Config } | { kind: 'save'; id: string | null; name: string; config: Config } | { kind: 'rename'; id: string; name: string } | { kind: 'load'; id: string } | { kind: 'undo' }
export class ConsoleSession {
  api: Api
  snapshot: Snapshot | null = null
  local: Config | null = null
  baseline: string = signature(null)
  inventory: Inventory | null = null
  status: Status | null = null
  inspection: Inspection | null = null
  validated = false
  busy = false
  error = ''
  statusError = ''
  notice = ''
  needsRefresh = true
  simulated = false
  constructor(api = new Api()) { this.api = api }
  get dirty() { return signature(this.local) !== this.baseline }
  get canWrite() { return !!this.snapshot && !this.busy && !this.needsRefresh }
  changed() { this.inspection = null; this.validated = false; this.notice = '' }
  replace(config: Config, simulated = false) { this.local = copy(readConfig(config)); this.simulated = simulated; this.changed() }
  async run(work: () => Promise<void>) {
    if (this.busy) return
    this.busy = true; this.error = ''; this.notice = ''
    try { await work() } catch (error) { this.error = error instanceof Error ? error.message : String(error) }
    finally { this.busy = false }
  }
  async refresh() {
    await this.run(async () => {
      this.needsRefresh = true
      this.status = null; this.statusError = ''
      await this.api.connect()
      const [store, status] = await Promise.allSettled([this.api.post('/api/profiles/read'), this.api.post('/api/status')])
      if (status.status === 'fulfilled') {
        try { this.status = readStatus(status.value) } catch (error) { this.statusError = String(error) }
      } else this.statusError = status.reason instanceof Error ? status.reason.message : String(status.reason)
      if (store.status === 'rejected') throw store.reason
      const snapshot = readSnapshot(store.value)
      // Reads never replace unsaved local edits, including reconnect/conflict recovery.
      if (!this.dirty) { this.local = copy(snapshot.draft); this.baseline = signature(this.local); this.changed(); this.simulated = false }
      this.snapshot = snapshot; this.needsRefresh = false
      this.notice = this.dirty ? 'Store refreshed. Local edits kept. Review the stored draft before choosing to save or reload.' : 'Stored draft refreshed.'
    })
  }
  reload() {
    if (!this.snapshot || this.needsRefresh || this.busy) return
    this.local = copy(this.snapshot.draft); this.baseline = signature(this.local); this.simulated = false; this.changed()
    this.notice = 'Stored draft reloaded.'
  }
  async edit(edit: Edit) {
    if (!this.canWrite || !this.snapshot) return
    const expectedRevision = this.snapshot.storeRevision
    await this.run(async () => {
      try {
        const snapshot = readSnapshot(await this.api.post('/api/profiles/edit', { expectedRevision, edit }))
        this.snapshot = snapshot
        if (['draft', 'load', 'undo'].includes(edit.kind)) {
          this.local = copy(snapshot.draft); this.baseline = signature(this.local); this.simulated = false; this.changed()
        }
        // Saving/renaming a profile does not persist or replace the stored draft.
        this.notice = edit.kind === 'save' ? 'Profile saved. Use Save draft to also reopen these edits as the draft.' : edit.kind === 'rename' ? 'Profile renamed.' : edit.kind === 'load' ? 'Profile loaded into the stored draft.' : edit.kind === 'undo' ? 'Previous stored draft restored.' : 'Draft saved. These edits will reopen from the store.'
      } catch (error) {
        this.needsRefresh = true
        if (error instanceof ApiError && error.status === 409) throw new Error(`Store conflict: ${error.message} Local edits kept. Refresh and review before saving again.`)
        throw error
      }
    })
  }
  async validate() {
    if (!this.local) return
    await this.run(async () => {
      this.validated = false
      const v = object(await this.api.post('/api/validate', { config: this.local }))
      if (v.valid !== true) throw new Error('Unsupported validation response.')
      this.validated = true; this.notice = 'Core validation passed. No audio was applied.'
    })
  }
  async inspect() {
    if (!this.local || !this.inventory) return
    await this.run(async () => {
      this.inspection = null
      const result = readInspection(await this.api.post('/api/inspect', { config: this.local, inventory: this.inventory }))
      if (result.configRevision !== this.local?.revision || result.inventoryRevision !== this.inventory?.revision) throw new Error('Inspection revisions do not match the current inputs.')
      this.inspection = result
    })
  }
}
