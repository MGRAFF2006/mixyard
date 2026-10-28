import { Api } from './api.ts'
import { copyDsp, dspJson } from './dsp-json.ts'
import { object } from './model.ts'
import { emptyRequest, parseSituation, readEvaluation, readLiveContext, readRequest } from './situations.ts'
import type { Evaluation, LiveContext, SituationRequest } from './situations.ts'
export class SituationSession {
  api: Pick<Api, 'post'>
  request = emptyRequest()
  evaluation: Evaluation | null = null
  live: LiveContext | null = null
  liveError = ''
  error = ''
  notice = ''
  validated = false
  busy = false
  collecting = false
  dirty = false
  // Generation includes invalid text drafts, not just successfully parsed edits.
  generation = 0
  constructor(api: Pick<Api, 'post'>) {
    this.api = api
  }
  invalidate() {
    this.generation++
    this.evaluation = null
    this.validated = false
    this.notice = ''
    this.error = ''
    this.dirty = true
  }
  edit(change: (request: SituationRequest) => void) {
    try {
      const next = copyDsp(this.request), previousGraph = dspJson(next.graph), previousCatalog = dspJson(next.catalog)
      change(next)
      if (dspJson(next.graph) !== previousGraph || dspJson(next.catalog) !== previousCatalog) {
        if (!Number.isSafeInteger(this.request.graph.revision + 1))
          throw new Error('Graph revision exceeds safe integer range. Export and start a fresh graph revision explicitly.')
        next.graph.revision = this.request.graph.revision + 1
      }
      if (dspJson(next.snapshot) !== dspJson(this.request.snapshot) && next.snapshot.epoch === this.request.snapshot.epoch && next.snapshot.revision === this.request.snapshot.revision) {
        if (!Number.isSafeInteger(next.snapshot.revision + 1))
          throw new Error('Snapshot revision exceeds safe integer range.')
        next.snapshot.revision++
      }
      this.invalidate()
      this.request = next
    }
    catch (error) {
      this.fail(error)
    }
  }
  fail(error: unknown) {
    this.error = error instanceof Error ? error.message : String(error)
  }
  private async validateRequest(request: SituationRequest) {
    const result = object(await this.api.post('/api/automation/validate', { graph: request.graph, catalog: request.catalog }))
    if (result.valid !== true)
      throw new Error('Core did not validate this graph.')
  }
  async validate() {
    if (this.busy)
      return
    this.busy = true
    this.error = ''
    this.evaluation = null
    this.validated = false
    const generation = this.generation
    try {
      const request = copyDsp(readRequest(this.request))
      await this.validateRequest(request)
      if (generation !== this.generation)
        return
      this.validated = true
      this.notice = 'Core graph validation passed. Snapshot claims have not been evaluated.'
    }
    catch (error) {
      if (generation === this.generation)
        this.fail(error)
    }
    finally {
      this.busy = false
    }
  }
  async evaluate() {
    if (this.busy)
      return
    this.validated = false
    this.busy = true
    this.error = ''
    this.evaluation = null
    this.notice = ''
    const generation = this.generation
    try {
      const request = copyDsp(readRequest(this.request)), signature = dspJson(request)
      const result = readEvaluation(await this.api.post('/api/automation/evaluate', request), request)
      if (generation !== this.generation || signature !== dspJson(this.request))
        return
      this.evaluation = result
      this.validated = true
    }
    catch (error) {
      if (generation === this.generation)
        this.fail(error)
    }
    finally {
      this.busy = false
    }
  }
  async import(text: string) {
    if (this.busy)
      return false
    this.busy = true
    this.error = ''
    this.notice = ''
    this.evaluation = null
    this.validated = false
    const generation = this.generation
    try {
      const request = copyDsp(parseSituation(text))
      await this.validateRequest(request)
      if (generation !== this.generation)
        return false
      // Validate the snapshot and complete request as well, with no side effects.
      readEvaluation(await this.api.post('/api/automation/evaluate', request), request)
      if (generation !== this.generation)
        return false
      this.invalidate()
      this.request = request
      this.validated = true
      this.notice = 'Full configuration imported after core validation and simulation. Facts remain supplied claims. Export to keep this browser draft.'
      return true
    }
    catch (error) {
      if (generation === this.generation)
        this.fail(error)
      return false
    }
    finally {
      this.busy = false
    }
  }
  async collect() {
    if (this.collecting)
      return
    this.collecting = true
    this.liveError = ''
    this.live = null
    try {
      this.live = readLiveContext(await this.api.post('/api/automation/context', {}))
    }
    catch (error) {
      this.liveError = error instanceof Error ? error.message : String(error)
    }
    finally {
      this.collecting = false
    }
  }
  useLive() {
    if (!this.live)
      return
    const live = this.live
    this.edit(r => {
      r.snapshot = copyDsp(live.snapshot)
      r.nowMs = live.nowMs
      r.catalog = [...r.catalog.filter(d => !live.catalog.some(c => c.name === d.name)), ...copyDsp(live.catalog).map(d => ({ ...r.catalog.find(current => current.name === d.name), ...d }))]
    })
    this.notice = 'Read-only context copied into simulation inputs. Evaluation remains a simulation; changing claims does not establish native observations.'
  }
}
