import assert from 'node:assert/strict'
import test from 'node:test'
import { Api } from './api.ts'
import { DeviceTargets } from './targets.ts'
test('trusted additions preserve API/workspace ownership and removal retains dirty host for export', () => {
  const registry = new DeviceTargets(), a = new Api(), b = new Api(), replacement = new Api()
  const A = { id: 'A', displayName: 'A', api: a }, B = { id: 'B:credential-1', displayName: 'B', api: b }
  registry.reconcile([A], () => false)
  registry.reconcile([A, B], () => true)
  assert.equal(registry.current[0]!.api, a)
  assert.equal(a.epoch, 0)
  const retained = registry.reconcile([A], id => id === B.id)
  assert.equal(retained[1]!.api, b)
  assert.equal(retained[1]!.retired, true)
  assert.equal(b.epoch, 1)
  const next = registry.reconcile([A, { ...B, id: 'B:credential-2', api: replacement }], id => id === B.id)
  assert.equal(next[1]!.api, replacement)
  assert.equal(next[2]!.api, b)
  assert.equal(b.epoch, 1)
})
test('an existing target ID cannot silently bind a new native credential or API', () => {
  const registry = new DeviceTargets(), api = new Api()
  registry.reconcile([{ id: 'A', displayName: 'A', api }], () => false)
  assert.throws(() => registry.reconcile([{ id: 'A', displayName: 'A', api: new Api() }], () => false), /cannot be rebound/)
  assert.equal(registry.current[0]!.api, api)
})
