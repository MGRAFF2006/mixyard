import assert from 'node:assert/strict'
import { spawn, execFileSync } from 'node:child_process'
import type { ChildProcess } from 'node:child_process'
import { once } from 'node:events'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { createInterface } from 'node:readline'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'
import { Api } from '../../apps/web/src/api.ts'
import { pinnedFetch } from './pinned-fetch.ts'

const binary = process.env.MIXYARD_CONTROL_BINARY
if (!binary) throw new Error('Set MIXYARD_CONTROL_BINARY to the built control service; this integration suite never substitutes a mock server.')
const executable = resolve(binary)
const root = fileURLToPath(new URL('../../', import.meta.url))

class Device {
  directory: string
  child: ChildProcess | null = null
  trust: { deviceId: string; certificatePem: string }
  invitations: Record<string, string> = {}
  url = ''
  constructor(directory: string) {
    this.directory = directory
    mkdirSync(directory)
    mkdirSync(join(directory, 'assets'))
    // Canonical API construction requires assets, but the control service does
    // not expose static files. Reuse the repository's small real HTML entry.
    writeFileSync(join(directory, 'assets/index.html'), '<!doctype html><title>Owned control fixture</title>')
    this.trust = this.cli('init', '--ip', '127.0.0.1')
    for (const permissions of ['view,control,settings', 'view']) {
      this.invitations[permissions] = this.cli('invite', '--permissions', permissions, '--ttl-seconds', '300').invitation
    }
  }
  cli(...args: string[]) {
    // Keep invitation and credential material in memory, out of logs/artifacts.
    return JSON.parse(execFileSync(executable, ['--state', join(this.directory, 'identity'), ...args], { encoding: 'utf8', maxBuffer: 1024 * 1024 }))
  }
  async start() {
    assert.equal(this.child, null)
    const child = spawn(executable, ['--state', join(this.directory, 'identity'), 'serve', '--enable-control', '--bind', '127.0.0.1:0', '--store', join(this.directory, 'profiles'), '--assets', join(this.directory, 'assets'), '--instance', `control-${process.pid}-${this.directory.split('/').at(-1)}`], { stdio: ['ignore', 'pipe', 'inherit'] })
    this.child = child
    const lines = createInterface({ input: child.stdout! })
    let timer: ReturnType<typeof setTimeout> | undefined
    try {
      const line = await Promise.race([
        once(lines, 'line').then(([value]) => String(value)),
        once(child, 'exit').then(([code]) => { throw new Error(`Owned control process exited during startup (${code}).`) }),
        once(child, 'error').then(([error]) => { throw error }),
        new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error('Owned control startup deadline exceeded.')), 10_000) }),
      ])
      const readiness = JSON.parse(line)
      assert.equal(readiness.event, 'listening')
      assert.equal(readiness.tls, true)
      assert.equal(readiness.deviceId, this.trust.deviceId)
      assert.match(readiness.address, /^127\.0\.0\.1:\d+$/)
      this.url = `https://${readiness.address}`
    } finally { clearTimeout(timer); lines.close() }
  }
  async stop() {
    const child = this.child
    this.child = null
    if (!child || child.exitCode !== null || child.signalCode !== null) return
    const exited = once(child, 'exit')
    const timer = setTimeout(() => child.kill('SIGKILL'), 3000)
    try { child.kill(); await exited } finally { clearTimeout(timer) }
  }
  fetcher() { return pinnedFetch(this.url, this.trust.certificatePem) }
  async post(path: string, body: unknown, credential?: string) {
    return this.fetcher()(path, { method: 'POST', headers: { 'Content-Type': 'application/json', ...(credential ? { Authorization: `Bearer ${credential}` } : {}) }, body: JSON.stringify(body) })
  }
  async pair(permissions: string) {
    const invitation = this.invitations[permissions]
    const response = await this.post('/control/pair', { deviceId: this.trust.deviceId, invitation, name: `Owned ${permissions} controller` })
    assert.equal(response.status, 200)
    const value = await response.json()
    assert.equal(value.deviceId, this.trust.deviceId)
    assert.deepEqual([...value.permissions].sort(), permissions.split(',').sort())
    assert.equal(typeof value.credential, 'string')
    return value as { deviceId: string; credentialId: string; credential: string }
  }
  api(credential: string, fetcher = this.fetcher()) {
    return Api.forControl(fetcher, { deviceId: this.trust.deviceId, credential })
  }
}

test('real TLS service and shared Vue API isolate devices, authorities, revisions and revoked controllers', { timeout: 60_000 }, async () => {
  const directory = mkdtempSync('/dev/shm/mixyard-device-control-')
  const devices: Device[] = []
  try {
    const a = new Device(join(directory, 'a')); devices.push(a)
    const b = new Device(join(directory, 'b')); devices.push(b)
    assert.notEqual(a.trust.deviceId, b.trust.deviceId)
    await a.start(); await b.start()
    const ownerA = await a.pair('view,control,settings')
    const ownerB = await b.pair('view,control,settings')
    const viewer = await a.pair('view')
    assert.ok((await a.post('/control/pair', { deviceId: a.trust.deviceId, invitation: a.invitations.view, name: 'Replay' })).status >= 400, 'pairing grant cannot be reused')
    assert.equal((await a.post('/control/session', {})).status, 401, 'TLS without application authorization cannot administer a device')
    await assert.rejects(pinnedFetch(a.url, b.trust.certificatePem)('/control/session', { method: 'POST', body: '{}', headers: { 'Content-Type': 'application/json' } }), 'another device certificate must fail trust validation')

    const apiA = a.api(ownerA.credential), competing = a.api(ownerA.credential), apiB = b.api(ownerB.credential), view = a.api(viewer.credential)
    await apiA.connect(); await competing.connect(); await apiB.connect(); await view.connect()
    const initialA = await apiA.post('/api/profiles/read') as { storeRevision: number; draft: unknown }
    const initialB = await apiB.post('/api/profiles/read') as { storeRevision: number; draft: unknown }
    const config = JSON.parse(readFileSync(join(root, 'apps/web/src/example-config.json'), 'utf8'))
    await assert.rejects(view.post('/api/profiles/edit', { expectedRevision: initialA.storeRevision, edit: { kind: 'draft', config } }), 'view role cannot edit stored routing intent')
    const saved = await apiA.post('/api/profiles/edit', { expectedRevision: initialA.storeRevision, edit: { kind: 'draft', config } }) as { storeRevision: number; draft: unknown }
    assert.ok(saved.storeRevision > initialA.storeRevision)
    assert.deepEqual((await apiB.post('/api/profiles/read') as { draft: unknown }).draft, initialB.draft, 'editing A must not alter B')
    await assert.rejects(competing.post('/api/profiles/edit', { expectedRevision: initialA.storeRevision, edit: { kind: 'undo' } }), 'competing stale controller cannot overwrite current revision')
    await competing.connect()
    assert.deepEqual((await competing.post('/api/profiles/read') as { draft: unknown }).draft, config)

    // Delay a real response after the server has produced it. Reconnection must
    // prevent that old operation from being accepted into the new authority.
    const reached = Promise.withResolvers<void>(), release = Promise.withResolvers<void>()
    const real = a.fetcher()
    const held = a.api(ownerA.credential, async (input, init) => {
      const response = await real(input, init)
      if (input === '/control/request') { reached.resolve(); await release.promise }
      return response
    })
    await held.connect()
    const pending = held.post('/api/profiles/read')
    const rejected = assert.rejects(pending, 'a response from before reconnect must be discarded')
    try { await reached.promise; await held.connect() } finally { release.resolve() }
    await rejected

    const authority = await (await a.post('/control/session', {}, ownerA.credential)).json()
    assert.equal((await a.post('/control/request', { deviceId: b.trust.deviceId, authorityId: authority.authorityId, revision: authority.revision, path: '/api/profiles/read', body: {} }, ownerA.credential)).status, 409, 'explicit wrong-host request must fail')
    await a.stop(); await a.start()
    assert.equal((await a.post('/control/request', { deviceId: authority.deviceId, authorityId: authority.authorityId, revision: authority.revision, path: '/api/profiles/read', body: {} }, ownerA.credential)).status, 409, 'restart invalidates previous authority even with durable credentials')
    const restarted = a.api(ownerA.credential)
    await restarted.connect()
    assert.deepEqual((await restarted.post('/api/profiles/read') as { draft: unknown }).draft, config, 'stored intent survives service restart')
    const current = await (await a.post('/control/session', {}, ownerA.credential)).json()
    const revoked = await a.post('/control/revoke', { deviceId: current.deviceId, authorityId: current.authorityId, revision: current.revision, body: { credentialId: viewer.credentialId } }, ownerA.credential)
    assert.equal(revoked.status, 200)
    assert.equal((await a.post('/control/session', {}, viewer.credential)).status, 401, 'revoked controller cannot reconnect')
    assert.ok((await a.post('/control/request', { deviceId: current.deviceId, authorityId: current.authorityId, revision: current.revision, path: '/api/automation/arm', body: {} }, ownerA.credential)).status >= 400, 'remote transport cannot resurrect an automation approval')
    assert.ok((await a.post('/control/request', { deviceId: current.deviceId, authorityId: current.authorityId, revision: current.revision, path: '/api/not-a-real-admin-route', body: {} }, ownerA.credential)).status >= 400, 'route allowlist has no default administrative pass-through')
    assert.deepEqual((await apiB.post('/api/profiles/read') as { draft: unknown }).draft, initialB.draft)
  } finally {
    await Promise.all(devices.map(device => device.stop()))
    rmSync(directory, { recursive: true, force: true })
  }
})
