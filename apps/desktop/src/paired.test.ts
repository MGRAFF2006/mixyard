import assert from 'node:assert/strict'
import test from 'node:test'
import { pairedApi, nativeCredentialSentinel } from './paired.ts'
import type { PairedTarget } from './paired.ts'
import type { Invoke } from './transport.ts'
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
