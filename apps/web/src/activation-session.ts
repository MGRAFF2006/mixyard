import { ApiError } from './api.ts'
import type { Api } from './api.ts'
import { copyDsp, dspJson } from './dsp-json.ts'
import { defaultSettings, duration, parseSettings, readActivationStatus, readLive, readSettings, readSettingsSnapshot, REVISION_KEYS, sameAuthority } from './activation.ts'
import type { Live, ManualCommand, Review, Settings, SettingsSnapshot, Status } from './activation.ts'
import type { Definition, Graph } from './situations.ts'

export class ActivationSession {
  api: Pick<Api, 'post'>
  saved: SettingsSnapshot | null = null
  draft = defaultSettings()
  status: Status | null = null
  live: Live | null = null
  review: Review | null = null
  connected = false
  dirty = false
  imported = false
  needsRead = true
  busy = false
  controlBusy = false
  disarming = false
  refreshing = false
  error = ''
  notice = ''
  statusError = ''
  routingNotice = ''
  generation = 0
  private session = 0
  private draftGeneration = 0
  private authorityGeneration = 0
  private statusGeneration = 0
  private reviewDeadline = 0
  private now: () => number
  constructor(api: Pick<Api, 'post'>, now = () => Date.now()) { this.api = api; this.now = now }
  disconnect() {
    this.session++; this.generation++; this.draftGeneration++; this.authorityGeneration++; this.statusGeneration++
    this.invalidateReview(); this.saved = null; this.status = null; this.live = null
    this.connected = false; this.needsRead = true; this.busy = false; this.controlBusy = false
    this.disarming = false; this.refreshing = false; this.statusError = ''; this.notice = ''; this.error = ''
  }
  fail(error: unknown) { this.error = error instanceof ApiError ? `${error.code}: ${error.message}` : error instanceof Error ? error.message : String(error) }
  invalidateReview() { this.review = null; this.reviewDeadline = 0 }
  builderChanged() {
    this.generation++
    this.invalidateReview()
    this.live = null
  }
  changed() {
    this.generation++
    this.draftGeneration++
    this.dirty = true
    this.error = ''; this.notice = ''
    this.invalidateReview()
    this.live = null
  }
  edit(change: (s: Settings) => void) { this.changed(); change(this.draft) }
  get reviewCurrent() { return !!this.review && !!this.status && !this.needsRead && !this.dirty && sameAuthority(this.review.authority, this.status) && this.now() <= this.reviewDeadline }
  private acceptStatus(next: Status, reconcile = false) {
    if (this.status && !reconcile && REVISION_KEYS.some(k => next[k] !== null && this.status![k] !== null && next[k]! < this.status![k]!)) throw Error('Stale server status discarded. Use console Refresh, then Read saved settings to reconnect to a restarted service.')
    if (this.review && (!sameAuthority(this.review.authority, next) || next.review?.id !== this.review.id || next.review.expiresAtMs !== this.review.expiresAtMs)) this.invalidateReview()
    if (this.status && next.storeRevision !== this.status.storeRevision) this.routingNotice = 'Stored routing changed. Your unsaved routing draft is kept. Use Refresh in the console to inspect the stored change.'
    if (this.saved && next.settingsRevision !== this.saved.settingsRevision) {
      this.needsRead = true
      this.invalidateReview()
      this.notice = 'Saved settings changed outside this editor. Read saved settings again; local edits are kept.'
    }
    this.status = next
    this.statusError = ''
  }
  async read(onAdopt?: () => void) {
    if (this.busy || this.controlBusy) return
    const session = this.session
    this.busy = true; this.error = ''; this.invalidateReview()
    const generation = this.draftGeneration, authority = ++this.authorityGeneration
    try {
      const saved = readSettingsSnapshot(await this.api.post('/api/automation/settings/read', {}))
      if (authority !== this.authorityGeneration) return
      if (this.saved && saved.settingsRevision < this.saved.settingsRevision) throw Error('Stale saved settings discarded.')
      this.saved = copyDsp(saved); this.connected = true; this.needsRead = false
      if (!this.dirty && generation === this.draftGeneration) {
        this.draft = copyDsp(saved.settings); this.imported = false
        onAdopt?.()
      }
      // Explicit reconciliation may attach to a restarted process, whose manual
      // revision begins again at zero. Prior requests/reviews have been revoked;
      // automatic polls still enforce monotonic status within this baseline.
      this.statusError = 'Service ownership is unconfirmed until reconciliation completes.'; this.live = null
      this.notice = this.dirty ? 'Fresh saved revision read. Local settings edits are kept; review them before saving.' : 'Saved settings read. Load the saved graph explicitly to replace the builder graph.'
      // Sequential status read: never start capture or activation on connection.
      await this.queryStatus(true)
    } catch (error) { if (authority === this.authorityGeneration) { this.needsRead = true; this.fail(error) } }
    finally { if (session === this.session) this.busy = false }
  }
  async refreshStatus() { await this.queryStatus() }
  private async queryStatus(reconcile = false) {
    if (!this.connected || (this.refreshing && !reconcile) || this.controlBusy) return
    this.refreshing = true
    const authority = this.authorityGeneration, refresh = ++this.statusGeneration
    try {
      const status = readActivationStatus(await this.api.post('/api/automation/status', {}))
      if (authority !== this.authorityGeneration) return
      this.acceptStatus(status, reconcile)
    } catch (error) {
      if (authority === this.authorityGeneration) { this.invalidateReview(); this.statusError = error instanceof Error ? error.message : String(error) }
    } finally { if (refresh === this.statusGeneration) this.refreshing = false }
  }
  import(text: string) {
    try {
      const imported = parseSettings(text)
      this.changed(); this.draft = copyDsp(imported.settings); this.imported = true
      this.notice = 'Imported local settings draft only. Imported revision is ignored. Nothing is saved, armed or observed.'
      return true
    } catch (error) { this.fail(error); return false }
  }
  async save(builder?: { graph: Graph; catalog: Definition[] }) {
    if (this.busy || this.controlBusy) return false
    if (!this.saved || this.needsRead) { this.fail('Read saved settings before another write. Local edits are kept.'); return false }
    this.error = ''; this.notice = ''; this.invalidateReview(); this.live = null
    let settings: Settings
    try {
      settings = copyDsp(readSettings(builder ? { ...this.draft, graph: builder.graph, catalog: builder.catalog } : this.draft))
      if (!Number.isSafeInteger(this.saved.settingsRevision + 1)) throw Error('Settings revision cannot be incremented exactly in this browser.')
    } catch (error) { this.fail(error); return false }
    const expected = this.saved.settingsRevision, generation = this.generation, authority = ++this.authorityGeneration
    const session = this.session
    this.busy = true
    // Any submitted write may have succeeded even if its acknowledgment is lost.
    this.needsRead = true
    try {
      const saved = readSettingsSnapshot(await this.api.post('/api/automation/settings/write', { expectedSettingsRevision: expected, settings }))
      if (authority !== this.authorityGeneration) return false
      if (saved.settingsRevision !== expected + 1) throw Error('Unexpected saved revision. Read saved settings before retrying.')
      this.saved = copyDsp(saved); this.needsRead = false
      if (generation === this.generation) { this.draft = copyDsp(saved.settings); this.dirty = false; this.imported = false }
      else this.dirty = true
      this.notice = 'Settings saved; automatic selection is disarmed. Review again before arming. Edits made during saving are kept.'
      await this.refreshStatus()
      return true
    } catch (error) {
      if (authority === this.authorityGeneration) { this.fail(error); this.notice = 'Write outcome needs a fresh read before another save. Local settings and builder edits are kept; no automatic retry.' }
      return false
    }
    finally { if (session === this.session) this.busy = false }
  }
  async capture(wantsReview: boolean) {
    if (this.busy || this.controlBusy) return
    if (!this.saved || this.needsRead || this.dirty) { this.fail('Save or reconcile settings before collecting a saved-policy review.'); return }
    if (!this.saved.settings.graph) { this.fail('Save builder graph and settings before reading a live decision.'); return }
    const session = this.session
    this.busy = true; this.error = ''; this.notice = ''; this.invalidateReview(); this.live = null
    const generation = this.generation, authority = ++this.authorityGeneration, saved = copyDsp(this.saved), started = this.now()
    try {
      const live = readLive(await this.api.post(`/api/automation/${wantsReview ? 'review' : 'live'}`, {}), saved, wantsReview)
      if (generation !== this.generation || authority !== this.authorityGeneration) return
      this.acceptStatus(live.status); this.live = live
      if (live.review) {
        this.review = live.review
        // Server clocks are epoch-local. Start conservatively at request dispatch.
        this.reviewDeadline = started + Math.min(300_000, live.review.expiresAtMs - live.request!.nowMs)
      }
      if (wantsReview && !live.review && !live.blocked) this.fail('No armable review returned. Inspect live details and retry review.')
    } catch (error) {
      if (generation === this.generation && authority === this.authorityGeneration) this.fail(error)
    } finally { if (session === this.session) this.busy = false }
  }
  async arm() {
    if (this.busy || this.controlBusy) return
    if (!this.reviewCurrent) { this.invalidateReview(); this.fail('Review expired or authority changed. Create a new concrete review before arming.'); return }
    await this.control('arm', { reviewId: this.review!.id })
  }
  async manual(command: ManualCommand) {
    if (command.mode === 'for') {
      try { duration(command.durationMs, 1) } catch (error) { this.fail(error); return }
    }
    await this.control('manual', command)
  }
  async disarm() { await this.control('disarm', {}) }
  private async control(path: string, body: unknown) {
    if (!this.connected || (path === 'disarm' ? this.disarming : this.controlBusy)) return
    this.disarming = path === 'disarm'
    this.controlBusy = true; this.error = ''; this.notice = ''; this.invalidateReview(); this.live = null
    const authority = ++this.authorityGeneration
    try {
      const status = readActivationStatus(await this.api.post(`/api/automation/${path}`, body))
      if (authority !== this.authorityGeneration) return
      this.acceptStatus(status)
      if (path === 'arm' && !status.armed) throw Error('Service did not confirm armed authority.')
      if (path === 'disarm' && status.armed) throw Error('Service did not confirm disarming. Refresh status and retry Disarm.')
      this.notice = path === 'disarm' ? 'Automatic selection disarmed. Pending capture is cancelled.' : path === 'arm' ? 'Automatic selection armed for the reviewed stored intent. Audio is not applied.' : 'Manual ownership updated. Clearing a pause resumes only authority that was already armed; fresh capture and settling still apply.'
    } catch (error) {
      if (authority === this.authorityGeneration) {
        this.fail(error)
        this.statusError = 'Ownership change is unconfirmed. Refresh status before another action; Disarm remains available.'
      }
    } finally {
      // A superseded Arm/manual reply must not unlock a newer Disarm request.
      if (authority === this.authorityGeneration) { this.controlBusy = false; this.disarming = false }
    }
  }
}
