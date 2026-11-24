import { test } from 'node:test'
import assert from 'node:assert/strict'
import { MobileBridge, readSession, encodeNativeMessage } from './bridge.ts'
import { Api } from '../../../web/src/api.ts'
import { dspJson } from '../../../web/src/dsp-json.ts'
const session = { deviceId: 'phone-controlled-host', authorityId: 'a1', revision: 0, permissions: ['settings'], backendAvailable: false, audioApplied: false }
const envelope = (path = '/api/profiles/edit') => ({ deviceId: session.deviceId, authorityId: session.authorityId, revision: 0, path, body: { expectedRevision: 0 } })
const nativeReply = (value: unknown, status = 200) => ({ status, json: dspJson(value) })
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
    return method === 'session' ? session : nativeReply({ ...session, revision: 1, status: 200, body: { ok: true } })
  })
  const api = Api.forControl(bridge.fetch, { deviceId: session.deviceId, credential: 'native-owned' })
  await api.connect()
  assert.equal(api.identity?.authorityId, 'a1')
  assert.deepEqual(await api.post('/api/profiles/edit', { expectedRevision: 0 }), { ok: true })
  assert.deepEqual(calls, [['session', {}], ['request', dspJson(envelope())]])
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
  finish(nativeReply({ ...session, revision: 1, status: 200, body: { ok: true } }))
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
  finish(nativeReply({ ...session, revision: 1, status: 200, body: {} }))
  assert.equal((await first).status, 200)
  await assert.rejects(second, /before dispatch/)
  assert.equal(writes, 1)
})
test('parallel console refresh reads serialize safely at the same revision', async () => {
  let count = 0
  const bridge = new MobileBridge(async method => method === 'session' ? session : (++count, nativeReply({ ...session, status: 200, body: {} })))
  const api = Api.forControl(bridge.fetch, { deviceId: session.deviceId, credential: 'native-owned' })
  await api.connect()
  await Promise.all([api.post('/api/profiles/read'), api.post('/api/status')])
  assert.equal(count, 2)
})
test('old authority cannot revive session and identity spoof is denied before native dispatch', async () => {
  const bridge = new MobileBridge(async method => method === 'session' ? session : nativeReply({ ...session, authorityId: 'restarted', status: 200, body: {} }))
  await bridge.fetch('/control/session')
  assert.equal((await bridge.fetch('/control/request', { method: 'POST', body: JSON.stringify({ ...envelope(), deviceId: 'other' }) })).status, 409)
  await assert.rejects(bridge.fetch('/control/request', request()), /authority/)
  assert.equal((await bridge.fetch('/control/request', request())).status, 401)
})
test('canonical permission denial is preserved by the actual shared Api', async () => {
  const bridge = new MobileBridge(async method => method === 'session' ? session : nativeReply({ ...session, status: 403, body: { error: { code: 'permission-denied', message: 'view required' } } }, 403))
  const api = Api.forControl(bridge.fetch, { deviceId: session.deviceId, credential: 'native-owned' })
  await api.connect()
  await assert.rejects(api.post('/api/status'), /view required/)
  await assert.rejects(api.post('/api/automation/arm'), /unsupported/)
})

test('raw canonical requests and replies preserve signed zero and finite Float64 bits through the native message boundary', async () => {
  const samples = [-0, Number.MIN_VALUE, Number.MAX_VALUE, 0.10000000000000002]
  const bridge = new MobileBridge(async (method, payload) => {
    if (method === 'session') return session
    assert.equal(typeof payload, 'string')
    const message = JSON.parse(encodeNativeMessage(method, payload, 1))
    assert.equal(message.payload, payload)
    const request = JSON.parse(message.payload)
    request.body.signal.samples.forEach((value: number, i: number) => assert.ok(Object.is(value, samples[i])))
    const reply = nativeReply({ ...session, status: 200, body: { samples: request.body.signal.samples } })
    // Android's outer JSONObject/WebMessage and WebKit carry only metadata and
    // the original raw string, never parsed canonical numeric payloads.
    return JSON.parse(JSON.stringify({ value: reply })).value
  })
  const api = Api.forControl(bridge.fetch, { deviceId: session.deviceId, credential: 'native-owned' })
  await api.connect()
  const result = await api.post('/api/dsp/preview', { signal: { samples } }) as { samples: number[] }
  result.samples.forEach((value, i) => assert.ok(Object.is(value, samples[i])))
  assert.throws(() => encodeNativeMessage('request', envelope(), 1), /raw JSON/)
})

test('real unwrapped permission denial preserves other grants; unauthorized response revokes shared authority', async () => {
  let calls = 0
  const bridge = new MobileBridge(async (method, payload) => {
    if (method === 'session') return { ...session, permissions: ['view'] }
    calls++
    const request = JSON.parse(String(payload))
    if (request.path === '/api/profiles/edit') return nativeReply({ error: { code: 'permission-denied', message: 'Control was not granted' } }, 403)
    if (calls < 3) return nativeReply({ ...session, status: 200, body: { connected: false } })
    return nativeReply({ error: { code: 'unauthorized', message: 'Controller revoked' } }, 401)
  })
  const api = Api.forControl(bridge.fetch, { deviceId: session.deviceId, credential: 'native-owned' })
  await api.connect()
  await assert.rejects(api.post('/api/profiles/edit'), /Control was not granted/)
  assert.equal(api.connected, true)
  assert.deepEqual(await api.post('/api/status'), { connected: false })
  assert.equal(calls, 2)
  await assert.rejects(api.post('/api/status'), /Controller revoked/)
  assert.equal(api.connected, false)
})

test('native transport failures immediately clear the shared Api instead of leaving stale connected authority', async () => {
  const bridge = new MobileBridge(async method => {
    if (method === 'session') return session
    throw new Error('Native TLS transport failed')
  })
  const api = Api.forControl(bridge.fetch, { deviceId: session.deviceId, credential: 'native-owned' })
  await api.connect()
  await assert.rejects(api.post('/api/status'), /offline or timed out/)
  assert.equal(api.connected, false)
})

test('scoped revocation clears mobile host notifications and shared authority; permission denial retains both', async () => {
  for (const [status, code, revoked] of [[409, 'store-conflict', true], [401, 'unauthorized', true], [403, 'invalid-origin', true], [403, 'permission-denied', false]] as const) {
    const notifications: unknown[] = []
    let dispatched = 0
    const bridge = new MobileBridge(async method => {
      if (method === 'session') return session
      dispatched++
      return nativeReply({ ...session, status, body: { error: { code, message: 'Original canonical reason' } } }, status)
    }, value => { notifications.push(value) })
    const api = Api.forControl(bridge.fetch, { deviceId: session.deviceId, credential: 'native-owned' })
    await api.connect()
    await assert.rejects(api.post('/api/automation/settings/write'), /Original canonical reason/)
    assert.equal(api.connected, !revoked)
    assert.equal(notifications.at(-1) === null, revoked)
    if (revoked) {
      assert.equal((await bridge.fetch('/control/request', request())).status, 401)
      await assert.rejects(api.post('/api/automation/settings/read'), /Refresh to reconnect/)
    }
    assert.equal(dispatched, 1)
  }
})
