import assert from 'node:assert/strict'
import test from 'node:test'
import { pairedApi, nativeCredentialSentinel } from './paired.ts'
import type { PairedTarget } from './paired.ts'
import type { Invoke } from './transport.ts'
import { createRequire } from 'node:module'
import { readFileSync } from 'node:fs'
import { ConsoleSession } from '../../web/src/session.ts'
import { readConfig, signature } from '../../web/src/model.ts'
const { watch } = createRequire(new URL('../../web/package.json', import.meta.url))('vue') as typeof import('vue')
const target: PairedTarget = { deviceId: 'device-B', displayName: 'Device B', endpoint: 'https://127.0.0.1:1', certificateSha256: 'pin', credentialId: 'controller', permissions: ['view', 'control'] }
const identity = { deviceId: target.deviceId, authorityId: 'boot-B', revision: 0, permissions: target.permissions, backendAvailable: false, audioApplied: false }

test('shared Api reaches native authority using only the sentinel and exact signed-zero envelope', async () => {
  const calls: { command: string; args?: Record<string, unknown> }[] = []
  const invoke: Invoke = async <T>(command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args })
    if (command === 'paired_open') return { generation: 1, status: 200, body: JSON.stringify(identity) } as T
    if (command === 'paired_request') return { status: 200, body: '{"deviceId":"device-B","authorityId":"boot-B","revision":1,"status":200,"body":{"value":-0}}' } as T
    return undefined as T
  }
  const api = pairedApi(invoke, target)
  await api.connect()
  const response = await api.post('/api/automation/settings/write', { value: -0, finite: 1.0000000000000002 }) as { value: number }
  assert.ok(Object.is(response.value, -0))
  const sent = calls.find(call => call.command === 'paired_request')!
  assert.deepEqual(Object.keys(sent.args!).sort(), ['generation', 'raw', 'targetId'])
  assert.equal(sent.args!.raw, '{"deviceId":"device-B","authorityId":"boot-B","revision":0,"path":"/api/automation/settings/write","body":{"value":-0,"finite":1.0000000000000002}}')
  assert.ok(!String(sent.args!.raw).includes(nativeCredentialSentinel))
  assert.equal(api.identity!.revision, 1)
  api.disconnect()
  assert.ok(calls.some(call => call.command === 'paired_disconnect' && call.args?.expectedGeneration === 1))
})

test('host disconnect drops in-flight native completion and prevents a queued request from gaining new authority', async () => {
  let complete!: (reply: unknown) => void
  const pending = new Promise(resolve => { complete = resolve })
  const calls: string[] = []
  const invoke: Invoke = async <T>(command: string) => {
    calls.push(command)
    if (command === 'paired_open') return { generation: 4, status: 200, body: JSON.stringify(identity) } as T
    if (command === 'paired_request') return await pending as T
    return undefined as T
  }
  const api = pairedApi(invoke, target)
  await api.connect()
  const operation = api.post('/api/profiles/read')
  api.disconnect('Target switched')
  complete({ status: 200, body: JSON.stringify({ ...identity, status: 200, body: {} }) })
  await assert.rejects(operation, /session changed/i)
  assert.equal(api.identity, null)
  await assert.rejects(api.post('/api/profiles/edit'), /session unavailable/i)
  assert.equal(calls.filter(command => command === 'paired_request').length, 1)
})

test('permission denial retains the paired session; revocation requires a fresh session', async () => {
  let status = 403
  const invoke: Invoke = async <T>(command: string) => {
    if (command === 'paired_open') return { generation: 1, status: 200, body: JSON.stringify(identity) } as T
    return { status, body: JSON.stringify({ error: { code: status === 403 ? 'permission-denied' : 'unauthorized', message: 'Denied' } }) } as T
  }
  const api = pairedApi(invoke, target)
  await api.connect()
  await assert.rejects(api.post('/api/automation/settings/write'), /Denied/)
  assert.equal(api.connected, true)
  status = 401
  await assert.rejects(api.post('/api/profiles/read'), /Denied/)
  assert.equal(api.connected, false)
})

test('a scoped canonical settings conflict immediately clears shared and native authority without another dispatch', async () => {
  let nativeConnected = true, requests = 0
  const invoke: Invoke = async <T>(command: string) => {
    if (command === 'paired_open') return { generation: 1, status: 200, body: JSON.stringify(identity) } as T
    if (command === 'paired_disconnect') { nativeConnected = false; return undefined as T }
    requests++; nativeConnected = false
    return { status: 409, body: JSON.stringify({ ...identity, status: 409, body: { error: { code: 'store-conflict', message: 'Read fresh settings before retrying' } } }) } as T
  }
  const api = pairedApi(invoke, target)
  await api.connect()
  const epoch = api.epoch
  await assert.rejects(api.post('/api/automation/settings/write', { expectedSettingsRevision: 0 }), /Read fresh settings/)
  assert.equal(nativeConnected, false)
  assert.equal(api.connected, false)
  assert.equal(api.identity, null)
  assert.equal(api.epoch, epoch + 1)
  await assert.rejects(api.post('/api/automation/settings/read'), /Refresh to reconnect/)
  assert.equal(requests, 1)
})

for (const operation of ['open', 'request'] as const) for (const outcome of ['success', 'rejection'] as const) {
  test(`late native ${operation} ${outcome} cannot invalidate a newer paired workspace`, async () => {
    let release!: (reply: unknown) => void, reject!: (error: unknown) => void, arrived!: () => void
    const held = new Promise((resolve, fail) => { release = resolve; reject = fail })
    const dispatched = new Promise<void>(resolve => { arrived = resolve })
    let opens = 0, active: number | null = null, heldRequest = false
    const disconnects: number[] = [], requests: number[] = []
    const config = readConfig(JSON.parse(readFileSync(new URL('../../web/src/example-config.json', import.meta.url), 'utf8')))
    const store = { format: 'mixyard-local-drafts', schemaVersion: 1, storeRevision: 0, configRevision: 1, profiles: [], draft: config, previousDraft: null }
    const invoke: Invoke = async <T>(command: string, args?: Record<string, unknown>) => {
      if (command === 'paired_open') {
        active = ++opens
        if (operation === 'open' && opens === 1) { arrived(); return await held as T }
        return { generation: opens, status: 200, body: JSON.stringify(identity) } as T
      }
      if (command === 'paired_disconnect') {
        const expected = args!.expectedGeneration as number
        disconnects.push(expected); if (active === expected) active = null
        return undefined as T
      }
      assert.equal(command, 'paired_request')
      assert.equal(args!.generation, active, 'shared requests must reach the still-current native generation')
      requests.push(args!.generation as number)
      const path = JSON.parse(args!.raw as string).path
      if (operation === 'request' && path === '/api/validate' && !heldRequest) {
        heldRequest = true; arrived(); return await held as T
      }
      const body = path === '/api/profiles/read' ? store : path === '/api/status' ? { connected: false, snapshot: null, error: null } : { valid: true }
      return { status: 200, body: JSON.stringify({ ...identity, status: 200, body }) } as T
    }
    const api = pairedApi(invoke, target), workspace = new ConsoleSession(api)
    const stop = watch(() => api.epoch, () => workspace.invalidate(), { flush: 'sync' })
    try {
      let old: Promise<unknown>
      if (operation === 'open') old = api.connect().then(() => null, error => error)
      else {
        await workspace.refresh(); assert.equal(workspace.canWrite, true)
        workspace.local!.sends[0]!.gain = 0.37; workspace.changed()
        old = workspace.validate()
      }
      await dispatched
      api.disconnect('Explicit disconnect before reconnect')
      await workspace.refresh(); assert.equal(api.connected, true); assert.equal(workspace.canWrite, true)
      await workspace.validate(); assert.equal(workspace.validated, true)
      const draft = signature(workspace.local), epoch = api.epoch
      if (outcome === 'rejection') reject('Owned obsolete native transport rejection')
      else release(operation === 'open' ? { generation: 1, status: 200, body: JSON.stringify(identity) }
        : { status: 200, body: JSON.stringify({ ...identity, status: 200, body: { valid: true } }) })
      await old
      assert.equal(active, 2, 'stale cleanup must not disconnect the new native generation')
      assert.equal(api.epoch, epoch); assert.equal(api.connected, true)
      assert.equal(workspace.canWrite, true); assert.equal(workspace.validated, true)
      assert.equal(signature(workspace.local), draft)
      assert.ok(!disconnects.includes(2), 'obsolete operation may clean only its own generation')
      const before = requests.length
      await api.post('/api/profiles/read')
      assert.equal(requests.length, before + 1, 'a new read actually dispatches through native generation2')
      assert.equal(requests.at(-1), 2)
    } finally { stop(); release({}); api.disconnect() }
  })
}
