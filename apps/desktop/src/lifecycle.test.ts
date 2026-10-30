import assert from 'node:assert/strict'
import test from 'node:test'
import { closeInterface } from './lifecycle.ts'
import type { Invoke } from './transport.ts'

test('cancelled loss of drafts prevents every exiting native command', async () => {
  const calls: string[] = []
  const invoke: Invoke = async <T>(command: string) => { calls.push(command); return undefined as T }
  for (const platform of ['linux', 'windows', null]) {
    assert.equal(await closeInterface('hide_mixer', platform, () => true, () => false, invoke), false)
  }
  assert.equal(await closeInterface('quit_ui', 'macos', () => true, () => false, invoke), false)
  assert.deepEqual(calls, [])
})

test('clean close and macOS hide do not prompt; explicit confirmed Quit invokes only UI exit', async () => {
  const calls: string[] = []
  const invoke: Invoke = async <T>(command: string) => { calls.push(command); return undefined as T }
  const noPrompt = () => { throw new Error('unexpected loss prompt') }
  assert.equal(await closeInterface('hide_mixer', 'macos', () => true, noPrompt, invoke), true)
  assert.equal(await closeInterface('hide_mixer', 'linux', () => false, noPrompt, invoke), true)
  assert.equal(await closeInterface('quit_ui', 'windows', () => true, () => true, invoke), true)
  assert.deepEqual(calls, ['hide_mixer', 'hide_mixer', 'quit_ui'])
})
