import assert from 'node:assert/strict'
import { spawn, type ChildProcess } from 'node:child_process'
import { createInterface } from 'node:readline'
import { dspJson } from '../../apps/web/src/dsp-json.ts'

export type NativeReply = { ok: boolean; raw?: string; status?: number; hasAuthority: boolean; failure?: string; recordRetained?: boolean; systemCertificateObservedTrusted?: boolean; privateAnchorRejected?: boolean; evaluatorOnly?: boolean }
export class Native {
  readonly child: ChildProcess
  private terminal = false
  private phase = 'spawn'
  private firstProblem: string | null = null
  private isReady = false
  private pending: { resolve: (value: NativeReply) => void; reject: (error: Error) => void } | null = null
  private prepared: Promise<void>
  private exited: Promise<number | null>
  private budget: number
  constructor(directory: string, command: string[], budget = 20_000) {
    this.budget = budget
    this.child = spawn(command[0]!, command.slice(1), { env: { ...process.env, MIXYARD_NATIVE_FIXTURE: directory }, stdio: ['pipe', 'pipe', 'pipe'] })
    let ready!: () => void, failed!: (error: Error) => void
    this.prepared = new Promise<void>((resolve, reject) => { ready = resolve; failed = reject })
    void this.prepared.catch(() => {})
    const knownPhases = new Set(['startup-list', 'startup-create', 'startup-unlock', 'startup-search', 'store-a', 'store-b', 'store-viewer', 'trust-a', 'trust-b', 'trust-viewer', 'child-start', 'child-exit', 'cleanup-child', 'cleanup-search', 'cleanup-verify', 'cleanup-trust-a', 'cleanup-trust-b', 'cleanup-trust-viewer', 'cleanup-keychain', 'cleanup-recovery-retained', 'cleanup-complete', 'admin-before', 'admin-ready', 'admin-precleanup', 'admin-after', 'cleanup-command'].flatMap(base => ['start', 'done', 'failed'].map(status => `${base}-${status}`)))
    let stderr = '', phases = 0, reasons = 0
    this.child.stderr!.on('data', data => {
      // Drain all bytes, but expose only our small fixed phase vocabulary.
      const lines = (stderr + String(data)).split('\n'); stderr = lines.pop()!.slice(-4096)
      for (const line of lines) {
        if (line.startsWith('MIXYARD_OWNED_RECEIPT ') && line.length < 512) {
          try {
            const value = JSON.parse(line.slice('MIXYARD_OWNED_RECEIPT '.length))
            if (Object.keys(value).sort().join(',') === 'descendants,directChildReaped,phase,pid,pipesEof,returncode' &&
                ['privileged-timeout-reaped','privileged-cleanup-reaped'].includes(value.phase) &&
                value.directChildReaped === true && value.pipesEof === true && value.descendants === 'unproved' &&
                Number.isSafeInteger(value.pid) && value.pid >= 0 && value.pid < 2 ** 31 &&
                Number.isSafeInteger(value.returncode) && Math.abs(value.returncode) < 2 ** 31 && reasons++ < 16) {
              process.stderr.write(`Owned direct wait/EOF receipt (NOT trust or descendant proof): ${JSON.stringify(value)}\n`)
            }
          } catch { /* No raw subprocess streams, paths or inferred global stop. */ }
          continue
        }
        if (line.startsWith('MIXYARD_OWNED_SIGNAL ') && line.length < 512) {
          try {
            const value = JSON.parse(line.slice('MIXYARD_OWNED_SIGNAL '.length))
            if (Object.keys(value).sort().join(',') === 'errno,phase,pid,result' &&
                ['before-privileged-term','cleanup-privileged-term'].includes(value.phase) &&
                ['accepted','os-error'].includes(value.result) && Number.isSafeInteger(value.pid) && value.pid >= 0 && value.pid < 2 ** 31 &&
                Number.isSafeInteger(value.errno) && value.errno >= 0 && value.errno < 2 ** 31 && reasons++ < 16) {
              process.stderr.write(`Owned terminate syscall result (NOT completion): ${JSON.stringify(value)}\n`)
            }
          } catch { /* Fixed syscall/errno only; never exception text or argv. */ }
          continue
        }
        if (line.startsWith('MIXYARD_OWNED_PROCESS ') && line.length < 4096) {
          try {
            const value = JSON.parse(line.slice('MIXYARD_OWNED_PROCESS '.length))
            if (Object.keys(value).sort().join(',') === 'phase,processes,state' &&
                ['before-privileged-term','cleanup-privileged-term'].includes(value.phase) &&
                ['unavailable','captured','owner-exited-pipes-unproved'].includes(value.state) &&
                Array.isArray(value.processes) && value.processes.length <= 16 && value.processes.every((row: Record<string, unknown>) =>
                  Object.keys(row).sort().join(',') === 'comm,parentPid,pid,state,uid' && ['sudo','security'].includes(String(row.comm)) &&
                  ['pid','parentPid','uid'].every(key => Number.isSafeInteger(row[key]) && Number(row[key]) >= 0 && Number(row[key]) < 2 ** 31) &&
                  typeof row.state === 'string' && /^[A-Za-z+<>-]{1,8}$/.test(row.state)) && reasons++ < 16) {
              process.stderr.write(`Owned privileged process evidence: ${JSON.stringify(value)}\n`)
            }
          } catch { /* No argv, paths, environment or unrelated process data. */ }
          continue
        }
        if (line.startsWith('MIXYARD_TRUST_STATE ') && line.length < 256) {
          try {
            const value = JSON.parse(line.slice('MIXYARD_TRUST_STATE '.length))
            if (Object.keys(value).sort().join(',') === 'absent,adminCertificateStatus,phase,resolvedAdminCertificateCount,statuses' &&
                ['admin-before', 'admin-ready', 'admin-precleanup', 'admin-after'].includes(value.phase) && typeof value.absent === 'boolean' &&
                (value.adminCertificateStatus === 0 ? Number.isSafeInteger(value.resolvedAdminCertificateCount) && value.resolvedAdminCertificateCount >= 0 : value.resolvedAdminCertificateCount === null) && Number.isSafeInteger(value.adminCertificateStatus) && Math.abs(value.adminCertificateStatus) < 2 ** 31 &&
                Array.isArray(value.statuses) && value.statuses.length === 3 && value.statuses.every((code: unknown) => Number.isSafeInteger(code) && Number(code) > -(2 ** 31) && Number(code) < 2 ** 31) && reasons++ < 16) {
              process.stderr.write(`Owned admin trust readback: ${value.phase} absent=${value.absent} statuses=${value.statuses.join(',')} resolvedAdminCertificateCount=${value.resolvedAdminCertificateCount === null ? 'unknown' : value.resolvedAdminCertificateCount} adminCertificateStatus=${value.adminCertificateStatus}\n`)
            }
          } catch { /* No raw trust state or arbitrary native output. */ }
          continue
        }
        if (line.startsWith('MIXYARD_SECURITY ') && line.length < 256) {
          try {
            const reason = JSON.parse(line.slice('MIXYARD_SECURITY '.length))
            const keys = Object.keys(reason).sort().join(',')
            const kinds = ['timeout', 'exit', 'signal', 'os-error', 'budget-exhausted', 'subprocess-error', 'stop-incomplete']
            if (keys === 'kind,phase,value' && typeof reason.phase === 'string' && knownPhases.has(`${reason.phase}-failed`) &&
                kinds.includes(reason.kind) && Number.isSafeInteger(reason.value) && reason.value >= 0 && reason.value < 2 ** 31 && reasons++ < 8) {
              process.stderr.write(`Owned Security failure: ${reason.phase} ${reason.kind} ${reason.value}\n`)
            }
          } catch { /* Unrecognized stderr is drained, never exposed. */ }
          continue
        }
        const match = /^MIXYARD_PHASE ([a-z0-9-]{1,64})$/.exec(line)
        if (!match || !knownPhases.has(match[1]!)) continue
        this.phase = match[1]!
        if (this.phase.endsWith('-failed') && !this.firstProblem) this.firstProblem = this.phase
        if (phases++ < 64) process.stderr.write(`Owned native phase: ${this.phase}\n`)
      }
    })
    const lines = createInterface({ input: this.child.stdout! })
    lines.on('line', line => {
      try {
        const value = JSON.parse(line)
        if (value.event === 'ready' && !this.isReady && Object.keys(value).length === 1) { this.isReady = true; this.phase = 'client-ready'; ready(); return }
        const pending = this.pending; this.pending = null
        if (!pending) throw new Error('Native driver replied without an admitted request')
        pending.resolve(value)
      } catch {
        const error = new Error('Native driver returned invalid or unsolicited JSONL')
        failed(error); this.pending?.reject(error); this.pending = null; this.terminal = true; this.child.kill('SIGTERM')
      }
    })
    this.child.stdin!.on('error', () => { this.pending?.reject(new Error('Native driver input closed')); this.pending = null })
    this.exited = new Promise(resolve => {
      this.child.on('error', () => {
        this.terminal = true; const error = new Error(`Native executable unavailable (phase ${this.phase})`)
        failed(error); this.pending?.reject(error); this.pending = null; resolve(null)
      })
      this.child.on('exit', (code, signal) => {
        this.terminal = true
        const error = new Error(`Native driver exited (code ${code}, signal ${signal}, phase ${this.firstProblem ?? this.phase})`)
        failed(error); this.pending?.reject(error); this.pending = null; lines.close(); resolve(code)
      })
    })
  }
  private wait<T>(value: Promise<T>, label: string, milliseconds: number): Promise<T> {
    let timer: NodeJS.Timeout
    return Promise.race([value, new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error(`${label} deadline (phase ${this.firstProblem ?? this.phase})`)), Math.max(0, milliseconds))
    })]).finally(() => clearTimeout(timer))
  }
  async call(body: unknown): Promise<NativeReply> {
    if (this.terminal) throw new Error(`Native driver is unavailable (phase ${this.phase})`)
    const deadline = performance.now() + this.budget
    try {
      // Preparation and RPC share the original20s total. Readiness does not
      // grant a longer operation deadline or send secrets before preparation.
      await this.wait(this.prepared, 'native readiness', deadline - performance.now())
      assert.equal(this.pending, null, 'test driver submits one exchange at a time')
      const reply = new Promise<NativeReply>((resolve, reject) => { this.pending = { resolve, reject } })
      const operation = body && typeof body === 'object' ? (body as { op?: unknown }).op : null
      this.phase = ['pair', 'invoke', 'capture', 'restartClient', 'storageFail', 'forget'].includes(String(operation)) ? `client-${operation}` : 'client-operation'
      this.child.stdin!.write(dspJson(body) + '\n')
      return await this.wait(reply, 'native client', deadline - performance.now())
    } catch (error) {
      process.stderr.write(`Owned native first failure phase: ${this.firstProblem ?? this.phase}\n`)
      this.terminal = true; this.pending = null; this.child.kill('SIGTERM'); throw error
    }
  }
  async ok(body: unknown): Promise<NativeReply> {
    const reply = await this.call(body)
    assert.equal(reply.ok, true, `Native operation failed (${reply.failure ?? 'unknown'})`)
    return reply
  }
  async session() {
    const reply = await this.ok({ op: 'invoke', method: 'session', payload: '{}' })
    assert.equal(typeof reply.raw, 'string', 'the shipping native client returns raw JSON')
    const value = JSON.parse(reply.raw!)
    assert.equal(value.backendAvailable, false); assert.equal(value.audioApplied, false)
    return value
  }
  async request(scope: any, path: string, body: unknown, expected?: string) {
    return this.call({ op: 'invoke', method: 'request', payload: dspJson({ deviceId: scope.deviceId, authorityId: scope.authorityId, revision: scope.revision, path, body }), ...(expected ? { expected } : {}) })
  }
  async close() {
    if (this.child.exitCode !== null || this.child.signalCode !== null) {
      assert.equal(await this.exited, 0, 'native driver and hosted trust cleanup must succeed')
      return
    }
    this.child.stdin!.end()
    const killBudget = Math.min(2000, this.budget / 4)
    try {
      const code = await this.wait(this.exited, 'native shutdown', this.budget - killBudget)
      assert.equal(code, 0, 'native driver and hosted trust cleanup must succeed')
    } catch (error) {
      this.child.kill('SIGKILL')
      await this.wait(this.exited, 'native reap', killBudget)
      throw error
    }
  }
}
