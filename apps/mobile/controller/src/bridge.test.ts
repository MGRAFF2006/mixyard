import { test } from 'node:test'
import assert from 'node:assert/strict'
import { MobileBridge, readSession } from './bridge.ts'
import { Api } from '../../../web/src/api.ts'
const session = { deviceId: 'phone-controlled-host', authorityId: 'a1', revision: 0, permissions: ['settings'], backendAvailable: false, audioApplied: false }
const envelope = (path = '/api/profiles/edit') => ({ deviceId: session.deviceId, authorityId: session.authorityId, revision: 0, path, body: { expectedRevision: 0 } })
const request = () => ({ method: 'POST', body: JSON.stringify(envelope()) })
test('settings is independent from view/control; unsafe revisions and audio claims fail', () => {
  assert.deepEqual(readSession(session).permissions, ['settings'])
  for (const revision of [-1, 0.5, 9007199254740992, '0', null]) assert.throws(() => readSession({ ...session, revision }))
  assert.throws(() => readSession({ ...session, backendAvailable: true }))
  assert.throws(() => readSession({ ...session, permissions: ['settings', 'settings'] }))
})
test('actual shared Api adapter exposes remote identity without a JS bearer secret', async () => {
  const calls: unknown[] = []
  const bridge = new MobileBridge(async (method, payload) => {
    calls.push([method, payload])
    return method === 'session' ? session : { ...session, revision: 1, status: 200, body: { ok: true } }
  })
  const api = Api.forControl(bridge.fetch, { deviceId: session.deviceId, credential: 'native-owned' })
  await api.connect()
  assert.equal(api.identity?.authorityId, 'a1')
  assert.deepEqual(await api.post('/api/profiles/edit', { expectedRevision: 0 }), { ok: true })
  assert.deepEqual(calls, [['session', {}], ['request', envelope()]])
  assert.equal(api.identity?.revision, 1)
  assert.equal((await bridge.fetch('https://attacker.invalid/control/request', request())).status, 403)
  assert.equal((await bridge.fetch('/api/session')).status, 403)
})
test('switch/revoke invalidates dispatched completion without replay', async () => {
  let finish!: (v: unknown) => void, writes = 0
  const bridge = new MobileBridge(async method => method === 'session' ? session : (++writes, new Promise(resolve => { finish = resolve })))
  await bridge.fetch('/control/session')
  const pending = bridge.fetch('/control/request', request())
  await Promise.resolve()
  bridge.invalidate()
  finish({ ...session, revision: 1, status: 200, body: { ok: true } })
  await assert.rejects(pending, /changed/)
  assert.equal(writes, 1)
  assert.equal((await bridge.fetch('/control/request', request())).status, 401)
})
test('queued write stays frozen and is discarded after earlier revision changes', async () => {
  let finish!: (v: unknown) => void, writes = 0
  const bridge = new MobileBridge(async method => method === 'session' ? session : (++writes, new Promise(resolve => { finish = resolve })))
  await bridge.fetch('/control/session')
  const first = bridge.fetch('/control/request', request())
  const second = bridge.fetch('/control/request', request())
  await Promise.resolve()
  finish({ ...session, revision: 1, status: 200, body: {} })
  assert.equal((await first).status, 200)
  await assert.rejects(second, /before dispatch/)
  assert.equal(writes, 1)
})
test('parallel console refresh reads serialize safely at the same revision', async () => {
  let count = 0
  const bridge = new MobileBridge(async method => method === 'session' ? session : (++count, { ...session, status: 200, body: {} }))
  const api = Api.forControl(bridge.fetch, { deviceId: session.deviceId, credential: 'native-owned' })
  await api.connect()
  await Promise.all([api.post('/api/profiles/read'), api.post('/api/status')])
  assert.equal(count, 2)
})
test('old authority cannot revive session and identity spoof is denied before native dispatch', async () => {
  const bridge = new MobileBridge(async method => method === 'session' ? session : { ...session, authorityId: 'restarted', status: 200, body: {} })
  await bridge.fetch('/control/session')
  assert.equal((await bridge.fetch('/control/request', { method: 'POST', body: JSON.stringify({ ...envelope(), deviceId: 'other' }) })).status, 409)
  await assert.rejects(bridge.fetch('/control/request', request()), /authority/)
  assert.equal((await bridge.fetch('/control/request', request())).status, 401)
})
test('canonical permission denial is preserved by the actual shared Api', async () => {
  const bridge = new MobileBridge(async method => method === 'session' ? session : { ...session, status: 403, body: { error: { code: 'permission-denied', message: 'view required' } } })
  const api = Api.forControl(bridge.fetch, { deviceId: session.deviceId, credential: 'native-owned' })
  await api.connect()
  await assert.rejects(api.post('/api/status'), /view required/)
  await assert.rejects(api.post('/api/automation/arm'), /unsupported/)
})
