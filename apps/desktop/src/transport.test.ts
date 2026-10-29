import assert from 'node:assert/strict'
import test from 'node:test'
import { desktopFetcher } from './transport.ts'
import type { Invoke } from './transport.ts'

test('native fetch preserves serialized precision and conflict status without retry', async () => {
  const calls: unknown[] = []
  const invoke: Invoke = async <T>(command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args }); return { status: 409, body: '{"error":{"code":"stale-revision"}}' } as T
  }
  const body = '{"nanoseconds":18446744073709551615}'
  const response = await desktopFetcher(invoke)('/api/dsp/delay', { method: 'POST', body, headers: { 'X-Mixyard-Token': 'a'.repeat(64) } })
  assert.equal(response.status, 409)
  assert.deepEqual(calls, [{ command: 'console_request', args: { request: { path: '/api/dsp/delay', method: 'POST', body, token: 'a'.repeat(64) } } }])
})

test('native fetch rejects remote origins and bearer credentials before native code', async () => {
  let calls = 0
  const invoke: Invoke = async () => { calls++; throw new Error('unexpected native operation') }
  const fetcher = desktopFetcher(invoke)
  await assert.rejects(fetcher('https://host/api/status'), /Remote control is unsupported/)
  await assert.rejects(fetcher('/control/request'), /Remote control is unsupported/)
  await assert.rejects(fetcher('/api/status', { headers: { Authorization: 'Bearer secret' } }), /does not forward authorization/)
  assert.equal(calls, 0)
})

test('aborted in-flight native write is not replayed or published as success', async () => {
  const controller = new AbortController()
  let calls = 0
  let complete!: (value: { status: number; body: string }) => void
  const pending = new Promise<{ status: number; body: string }>(resolve => { complete = resolve })
  const invoke: Invoke = async <T>() => { calls++; return await pending as T }
  const operation = desktopFetcher(invoke)('/api/profiles/edit', { method: 'POST', body: '{}', signal: controller.signal })
  controller.abort()
  complete({ status: 200, body: '{}' })
  await assert.rejects(operation, { name: 'AbortError' })
  assert.equal(calls, 1)
})
