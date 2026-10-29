import assert from 'node:assert/strict'
import { execFileSync, spawn, type ChildProcess } from 'node:child_process'
import { once } from 'node:events'
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { createServer, request } from 'node:https'
import { createPrivateKey } from 'node:crypto'
import { join, resolve } from 'node:path'
import { tmpdir } from 'node:os'
import { createInterface } from 'node:readline'
import { test } from 'node:test'
import { dspJson } from '../../apps/web/src/dsp-json.ts'

// A missing executable fails; these tests never replace the production service
// or the native client with a JavaScript implementation.
const binary = resolve(required('MIXYARD_CONTROL_BINARY'))
const command: string[] = JSON.parse(required('MIXYARD_NATIVE_COMMAND'))
assert.ok(Array.isArray(command) && command.length > 0 && command.every(x => typeof x === 'string'))
function required(name: string): string {
  const value = process.env[name]
  if (!value) throw new Error(`Set ${name}; no native/service facsimile is available.`)
  return value
}
function deadline<T>(promise: Promise<T>, label: string, ms = 20_000): Promise<T> {
  let timer: NodeJS.Timeout
  return Promise.race([promise, new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(`${label} deadline`)), ms)
  })]).finally(() => clearTimeout(timer))
}
type Trust = { deviceId: string; certificatePem: string; certificateSha256: string }
class Device {
  readonly directory: string
  readonly permissions: string
  trust: Trust
  invitations: string[] = []
  process: ChildProcess | null = null
  endpoint = ''
  constructor(directory: string, permissions: string) {
    this.directory = directory; this.permissions = permissions
    mkdirSync(directory)
    mkdirSync(join(directory, 'assets'))
    writeFileSync(join(directory, 'assets/index.html'), '<!doctype html><title>Owned native qualification</title>')
    this.trust = this.cli('init', '--ip', '127.0.0.1')
    for (let i = 0; i < 16; i++) this.invitations.push(this.cli('invite', '--permissions', permissions, '--ttl-seconds', '300').invitation)
    writeFileSync(join(directory, 'certificate.pem'), this.trust.certificatePem)
  }
  cli(...args: string[]) {
    // Private credentials, key material and invitations remain only in the
    // disposable fixture tree/memory, never test assertions or uploaded logs.
    return JSON.parse(execFileSync(binary, ['--state', join(this.directory, 'identity'), ...args], { encoding: 'utf8' }))
  }
  async start(port = '0') {
    const child = spawn(binary, ['--state', join(this.directory, 'identity'), 'serve', '--enable-control', '--bind', `127.0.0.1:${port}`, '--store', join(this.directory, 'profiles'), '--assets', join(this.directory, 'assets'), '--instance', `native-${process.pid}-${this.directory.split('/').at(-1)}`], { stdio: ['ignore', 'pipe', 'pipe'] })
    this.process = child
    const lines = createInterface({ input: child.stdout! })
    try {
      const [line] = await deadline(Promise.race([once(lines, 'line'), once(child, 'exit').then(() => { throw new Error('Control startup exited') }), once(child, 'error').then(() => { throw new Error('Control spawn failed') })]), 'control startup')
      const ready = JSON.parse(line)
      assert.equal(ready.tls, true)
      assert.equal(ready.deviceId, this.trust.deviceId)
      assert.match(ready.address, /^127\.0\.0\.1:\d+$/)
      this.endpoint = `https://${ready.address}`
    } finally { lines.close() }
  }
  async stop() {
    const child = this.process; this.process = null
    if (!child || child.exitCode !== null || child.signalCode !== null) return
    const exited = once(child, 'exit')
    const timer = setTimeout(() => child.kill('SIGKILL'), 10_000)
    try { child.kill('SIGTERM'); await deadline(exited, 'control shutdown', 15_000) } finally { clearTimeout(timer) }
  }
  provisioning() {
    const invitation = this.invitations.shift()
    assert.ok(invitation)
    return { op: 'pair', endpoint: this.endpoint, pin: this.trust.certificateSha256, deviceId: this.trust.deviceId, invitation }
  }
  async post(path: string, body: unknown, credential?: string): Promise<any> {
    return deadline(new Promise((resolveReply, reject) => {
      const req = request(new URL(path, this.endpoint), { method: 'POST', ca: this.trust.certificatePem, headers: { 'Content-Type': 'application/json', ...(credential ? { Authorization: `Bearer ${credential}` } : {}) } }, res => {
        const buffers: Buffer[] = []
        res.on('data', data => buffers.push(data))
        res.on('error', reject)
        res.on('end', () => resolveReply({ status: res.statusCode, body: JSON.parse(Buffer.concat(buffers).toString()) }))
      })
      req.on('error', reject); req.end(dspJson(body))
    }), 'test administrator')
  }
}
type NativeReply = { ok: boolean; raw?: string; status?: number; hasAuthority: boolean; failure?: string }
class Native {
  child: ChildProcess
  private terminal = false
  private pending: { resolve: (reply: NativeReply) => void; reject: (error: Error) => void } | null = null
  constructor(directory: string) {
    this.child = spawn(command[0], command.slice(1), { env: { ...process.env, MIXYARD_NATIVE_FIXTURE: directory }, stdio: ['pipe', 'pipe', 'pipe'] })
    const lines = createInterface({ input: this.child.stdout! })
    lines.on('line', line => {
      const pending = this.pending; this.pending = null
      if (!pending) return
      try { pending.resolve(JSON.parse(line)) } catch { pending.reject(new Error('Native driver returned invalid JSONL')) }
    })
    this.child.on('error', () => { this.terminal = true; this.pending?.reject(new Error('Native executable unavailable')); this.pending = null })
    this.child.on('exit', () => { this.terminal = true; this.pending?.reject(new Error('Native driver exited')); this.pending = null; lines.close() })
  }
  async call(body: unknown): Promise<NativeReply> {
    if (this.terminal) throw new Error('Native driver is unavailable')
    assert.equal(this.pending, null, 'test driver submits one exchange at a time')
    const reply = new Promise<NativeReply>((resolve, reject) => { this.pending = { resolve, reject } })
    this.child.stdin!.write(dspJson(body) + '\n')
    return deadline(reply, 'native client').catch(error => {
      this.terminal = true; this.pending = null; this.child.kill('SIGTERM'); throw error
    })
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
    if (this.child.exitCode !== null || this.child.signalCode !== null) return
    const exited = once(this.child, 'exit'); this.child.stdin!.end()
    try {
      const [code] = await deadline(exited, 'native shutdown', 20_000)
      assert.equal(code, 0, 'native driver and hosted trust cleanup must succeed')
    } catch (error) { this.child.kill('SIGKILL'); await exited; throw error }
  }
}
function envelope(reply: NativeReply, status = 200) {
  assert.equal(reply.ok, true, `Native request failed (${reply.failure ?? 'unknown'})`)
  assert.equal(typeof reply.raw, 'string')
  const value = JSON.parse(reply.raw!)
  assert.equal(reply.status, status, 'outer canonical HTTP status')
  assert.equal(value.status, status)
  assert.ok(Object.hasOwn(value, 'body'))
  return value
}

test('shipping native pinned control client against owned production TLS services', { timeout: 180_000 }, async t => {
  const directory = mkdtempSync(join(tmpdir(), 'mixyard-native-control-'))
  const devices: Device[] = []; let native: Native | undefined
  let redirect: ReturnType<typeof createServer> | undefined
  try {
    const a = new Device(join(directory, 'a'), 'view,control,settings'); devices.push(a)
    const b = new Device(join(directory, 'b'), 'view,control,settings'); devices.push(b)
    const viewer = new Device(join(directory, 'viewer'), 'view'); devices.push(viewer)
    // Adapter launches provision a private JVM truststore or a private hosted
    // macOS test keychain. Provisioning never changes this developer's PKI.
    await Promise.all(devices.map(device => device.start()))
    native = new Native(directory)
    const client = native
    let scope: any
    let saved: any
    let initialB: any
    await t.test('OOB pairing, immutable queued target and one-use invitation', async () => {
      const pairA = a.provisioning()
      await client.ok(pairA); scope = await client.session()
      assert.equal(scope.deviceId, a.trust.deviceId)
      const beforeA = envelope(await client.request(scope, '/api/profiles/read', {})).body
      const queuedEdit = { expectedRevision: beforeA.storeRevision, edit: { kind: 'draft', config: JSON.parse(readFileSync(new URL('../../apps/web/src/example-config.json', import.meta.url), 'utf8')) } }
      await client.ok({ op: 'capture', key: 'queued-a' })
      await client.ok(b.provisioning()); const scopeB = await client.session()
      const readB = envelope(await client.request(scopeB, '/api/profiles/read', {}))
      assert.equal(readB.deviceId, b.trust.deviceId); initialB = readB.body.draft
      assert.equal((await client.request(scope, '/api/profiles/edit', queuedEdit, 'queued-a')).ok, false)
      assert.deepEqual(envelope(await client.request(scopeB, '/api/profiles/read', {})).body.draft, initialB, 'queued A write never retargets B')
      assert.equal((await client.call(pairA)).ok, false, 'used invitation cannot pair twice')
      await client.ok(a.provisioning()); scope = await client.session()
    })
    await t.test('stored intent, exact raw Float64 body/reply and independent device', async () => {
      const initial = envelope(await client.request(scope, '/api/profiles/read', {}))
      const config = JSON.parse(readFileSync(new URL('../../apps/web/src/example-config.json', import.meta.url), 'utf8'))
      config.sends[0].gain = -0; config.sends[1].gain = 0.5000000000000001
      const edited = envelope(await client.request(scope, '/api/profiles/edit', { expectedRevision: initial.body.storeRevision, edit: { kind: 'draft', config } }))
      scope = { ...scope, revision: edited.revision }; saved = edited.body.draft
      assert.ok(Object.is(saved.sends[0].gain, -0), 'stored canonical signed zero survives the native serializer')
      assert.equal(saved.sends[1].gain, 0.5000000000000001)
      // These samples exercise the complete canonical request and response,
      // including nested arrays. This is offline DSP, never audio transport.
      const samples = [-0, 1.0000000000000002, 5e-324, -1.0000000000000002, -0.23456789012345678, 9007199254740992, Number.MAX_VALUE, -Number.MAX_VALUE]
      const rendered = envelope(await client.request(scope, '/api/dsp/preview', { config: { name: 'native precision', format: { sampleRateHz: 48001, channels: ['right', 'left'] }, effects: [] }, frames: 4, iterations: 1, bypassed: true, signal: { kind: 'samples', samples } }))
      assert.deepEqual(rendered.body.channels, ['right', 'left']); assert.equal(rendered.body.sampleRateHz, 48001)
      assert.equal(rendered.body.output.length, samples.length)
      samples.forEach((sample, index) => assert.ok(Object.is(rendered.body.output[index], sample), `Float64 bits at sample ${index}`))
      await client.ok({ op: 'restartClient' }); scope = await client.session()
      assert.deepEqual(envelope(await client.request(scope, '/api/profiles/read', {})).body.draft, saved)
      await client.ok(b.provisioning()); const scopeB = await client.session()
      assert.deepEqual(envelope(await client.request(scopeB, '/api/profiles/read', {})).body.draft, initialB, 'A stored writes do not alter B')
      await client.ok(a.provisioning()); scope = await client.session()
    })
    await t.test('service restart fences authority without replay and retains intent', async () => {
      const old = scope; const port = new URL(a.endpoint).port
      await a.stop(); await a.start(port)
      const stale = await client.request(old, '/api/profiles/read', {})
      assert.equal(stale.status, 409)
      assert.equal(stale.hasAuthority, false)
      scope = await client.session()
      assert.notEqual(scope.authorityId, old.authorityId)
      assert.deepEqual(envelope(await client.request(scope, '/api/profiles/read', {})).body.draft, saved)
    })
    await t.test('role 403 preserves session; durable revocation 401 clears it', async () => {
      const invite = viewer.provisioning()
      await client.ok(invite); const viewScope = await client.session()
      const denied = await client.request(viewScope, '/api/profiles/edit', { expectedRevision: 0, edit: { kind: 'undo' } })
      assert.equal(denied.ok, true); assert.equal(denied.status, 403)
      assert.equal(denied.hasAuthority, true, 'permission denied is not revocation')
      if (denied.raw) {
        const value = JSON.parse(denied.raw)
        assert.ok(value.error || value.status === 403)
      }
      envelope(await client.request(viewScope, '/api/profiles/read', {}))
      // Offline host revoke targets the credential ID in its own protected
      // fixture state. The test never asks native code to export a bearer.
      await viewer.stop()
      const state = JSON.parse(readFileSync(join(viewer.directory, 'identity/state.json'), 'utf8'))
      const id = (Object.values(state.credentials)[0] as { credentialId: string }).credentialId
      assert.ok(id)
      viewer.cli('revoke', '--credential-id', id)
      await viewer.start(new URL(invite.endpoint).port)
      const rejected = await client.request(viewScope, '/api/profiles/read', {})
      assert.equal(rejected.ok, true, `Native revoked reply must retain HTTP JSON (${rejected.failure ?? 'unknown'})`); assert.equal(rejected.status, 401)
      assert.equal(rejected.hasAuthority, false)
      const revoked = await client.call({ op: 'invoke', method: 'session', payload: '{}' })
      assert.equal(revoked.ok, false); assert.equal(revoked.hasAuthority, false)
    })
    await t.test('pin mismatch and PKI hostname mismatch never pair', async () => {
      assert.equal((await client.call({ ...a.provisioning(), pin: b.trust.certificateSha256 })).ok, false)
      // Production fixture certs include localhost/127.0.0.1. Use owned IPv6
      // loopback with that same trusted cert to make the hostname actually wrong.
      const state = JSON.parse(readFileSync(join(a.directory, 'identity/state.json'), 'utf8'))
      let connections = 0; let requests = 0
      const mismatch = createServer({ cert: a.trust.certificatePem, key: createPrivateKey({ key: Buffer.from(state.privateKeyPkcs8), type: 'pkcs8', format: 'der' }).export({ type: 'pkcs8', format: 'pem' }) }, (_req, res) => { requests++; res.writeHead(500); res.end() })
      mismatch.on('connection', () => { connections++ })
      try {
        mismatch.listen(0, '::1'); await once(mismatch, 'listening')
        const endpoint = `https://[::1]:${(mismatch.address() as any).port}`
        const hostname = await client.call({ ...a.provisioning(), endpoint })
        assert.equal(hostname.ok, false)
        assert.ok(connections > 0, 'hostname rejection must actually reach the owned TLS endpoint')
        assert.equal(requests, 0, 'wrong-host certificate must fail before sending an invitation')
      } finally { await new Promise<void>(resolveClose => mismatch.close(() => resolveClose())) }
    })
    await t.test('redirect denied without forwarding pairing invitation', async () => {
      const state = JSON.parse(readFileSync(join(a.directory, 'identity/state.json'), 'utf8'))
      let destinationRequests = 0
      let redirectRequests = 0
      redirect = createServer({ cert: a.trust.certificatePem, key: createPrivateKey({ key: Buffer.from(state.privateKeyPkcs8), type: 'pkcs8', format: 'der' }).export({ type: 'pkcs8', format: 'pem' }) }, (req, res) => {
        if (req.url === '/sink') { destinationRequests++; res.writeHead(500); res.end(); return }
        redirectRequests++
        res.writeHead(307, { Location: `https://127.0.0.1:${(redirect!.address() as any).port}/sink` }); res.end()
      })
      redirect.listen(0, '127.0.0.1'); await once(redirect, 'listening')
      const endpoint = `https://127.0.0.1:${(redirect.address() as any).port}`
      assert.equal((await client.call({ ...a.provisioning(), endpoint })).ok, false)
      assert.equal(redirectRequests, 1, 'the native client must actually reach the TLS redirect fixture')
      assert.equal(destinationRequests, 0)
    })
    await t.test('vault failure cannot replace persisted target; forgetting fails closed', async () => {
      await client.ok(a.provisioning()); scope = await client.session()
      await client.ok({ op: 'storageFail', enabled: true })
      assert.equal((await client.call(b.provisioning())).ok, false)
      await client.ok({ op: 'storageFail', enabled: false })
      await client.ok({ op: 'restartClient' }); scope = await client.session()
      assert.equal(scope.deviceId, a.trust.deviceId)
      assert.deepEqual(envelope(await client.request(scope, '/api/profiles/read', {})).body.draft, saved)
      await client.ok({ op: 'storageFail', enabled: true })
      const forgotten = await client.call({ op: 'forget' })
      assert.equal(forgotten.ok, false); assert.equal(forgotten.hasAuthority, false)
      await client.ok({ op: 'storageFail', enabled: false }); await client.ok({ op: 'forget' })
      assert.equal((await client.call({ op: 'invoke', method: 'session', payload: '{}' })).ok, false)
    })
    await t.test('uncertain POST and Retry-After response never replay', async () => {
      const state = JSON.parse(readFileSync(join(a.directory, 'identity/state.json'), 'utf8'))
      for (const mode of ['drop-response', 'retry-after-zero']) {
        let requests = 0
        const sink = createServer({ cert: a.trust.certificatePem, key: createPrivateKey({ key: Buffer.from(state.privateKeyPkcs8), type: 'pkcs8', format: 'der' }).export({ type: 'pkcs8', format: 'pem' }) }, (req, res) => {
          requests++; req.resume()
          req.on('end', () => {
            if (mode === 'drop-response') req.socket.destroy()
            else { res.writeHead(503, { 'Content-Type': 'application/json', 'Retry-After': '0' }); res.end('{"error":{"code":"owned-unavailable","message":"Owned negative replay fixture"}}') }
          })
        })
        try {
          sink.listen(0, '127.0.0.1'); await once(sink, 'listening')
          const endpoint = `https://127.0.0.1:${(sink.address() as any).port}`
          assert.equal((await client.call({ ...a.provisioning(), endpoint })).ok, false)
          assert.equal(requests, 1, `Native POST replayed after ${mode}`)
        } finally { await new Promise<void>(resolveClose => sink.close(() => resolveClose())) }
      }
    })
  } finally {
    await native?.close()
    if (redirect) await new Promise<void>(resolveClose => redirect!.close(() => resolveClose()))
    await Promise.all(devices.map(device => device.stop()))
    rmSync(directory, { recursive: true, force: true })
  }
})
