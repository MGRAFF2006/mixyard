import assert from 'node:assert/strict'
import { test } from 'node:test'
import { connectionLabel, disconnected, readConnection } from './runtime.ts'

const snapshot = () => ({ phase: 'connected', instance: 'default', error: null, event: {
  event: 'snapshot', sequence: 1, resync: true, lifecycle: 'running', historyAvailable: false,
  availability: { daemonReachable: true, backendAvailable: false, backend: 'unavailable',
    audioRevision: null, appliedRevision: null, desiredRevision: null, nativeAudioProof: 'unverified',
    capabilities: { apply: false, mute: false, nativeActionDispatch: false } },
} })
test('lifecycle connection is explicit, read-only and distinct from working audio', () => {
  assert.equal(connectionLabel(readConnection(disconnected())), 'Not connected')
  assert.equal(connectionLabel(readConnection(snapshot())), 'Connected · audio unavailable')
  assert.throws(() => readConnection({ ...disconnected(), phase: 'unknown' }))
  assert.throws(() => readConnection({ ...disconnected(), event: snapshot().event }))
  for (const field of ['backendAvailable', 'audioRevision', 'appliedRevision', 'desiredRevision']) {
    const state = snapshot()
    Object.assign(state.event.availability, { [field]: true })
    assert.throws(() => readConnection(state))
  }
  const state = snapshot()
  state.event.availability.capabilities.apply = true
  assert.throws(() => readConnection(state))
  assert.throws(() => readConnection({ ...snapshot(), instance: null }))
  const stopped = readConnection({ ...snapshot(), phase: 'stopped', event: { event: 'lifecycle', sequence: 2, lifecycle: 'stopping', backendAvailable: false } })
  assert.equal(connectionLabel(stopped), 'Runtime stopping · not connected')
})
