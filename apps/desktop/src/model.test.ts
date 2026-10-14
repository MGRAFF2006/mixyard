import assert from 'node:assert/strict'
import { test } from 'node:test'
import { blockedSend, controlPlayers, createPreview, needsMonitorConsent, newMix, newOutput, peakBound, previewPlayback, routedPlayers, updateSend } from './model.ts'

test('send gains are independent; disconnected edits and unsafe values are rejected', () => {
  const preview = createPreview()
  const [call, record, monitor] = preview.mixes
  assert.deepEqual(preview.mixes.map(mix => mix.sends['fixture:mic']!.gain), [1, 0.5, 0])
  assert.equal(updateSend('disconnected', call!, 'fixture:mic', { gain: 0.4 }), false)
  assert.equal(updateSend('demo', call!, 'fixture:mic', { gain: NaN }), false)
  assert.equal(updateSend('demo', call!, 'fixture:mic', { gain: 1.1 }), false)
  assert.equal(updateSend('demo', call!, 'fixture:mic', { gain: 0.4 }), true)
  assert.equal(record!.sends['fixture:mic']!.gain, 0.5)
  assert.equal(monitor!.sends['fixture:mic']!.gain, 0)
})

test('renaming never changes IDs or call-return exclusions', () => {
  const mix = createPreview().mixes[0]!
  const id = mix.id
  mix.name = 'Totally different name'
  assert.equal(mix.id, id)
  assert.equal(blockedSend(mix, 'fixture:return'), true)
  assert.equal(updateSend('demo', mix, 'fixture:return', { gain: 1 }), false)
  const custom = newMix('custom-id', 'Call')
  assert.equal(blockedSend(custom, 'fixture:return'), false)
  custom.name = 'Unrelated label'
  custom.outputs.push(newOutput('stable-route', 'fixture:call-input'))
  assert.equal(blockedSend(custom, 'fixture:return'), true)
  custom.outputs[0]!.name = 'Renamed output'
  custom.outputs[0]!.muted = true
  assert.equal(blockedSend(custom, 'fixture:return'), true)
})

test('dynamic mixes and many outputs have independent gain/mute, shared master', () => {
  const preview = createPreview()
  for (let i = 0; i < 50; i++) preview.mixes.push(newMix(`custom-${i}`, `Mix ${i}`))
  assert.equal(preview.mixes.length, 53)
  const mix = preview.mixes[2]!
  const [a, b] = mix.outputs
  const before = peakBound(mix, b)
  a!.gain = 0.5
  assert.equal(peakBound(mix, a), before * 0.5)
  assert.equal(peakBound(mix, b), before)
  a!.muted = true
  assert.equal(peakBound(mix, a), 0)
  assert.equal(peakBound(mix, b), before)
  mix.master *= 0.5
  assert.equal(peakBound(mix, b), before * 0.5)
  for (let i = 0; i < 50; i++) mix.outputs.push(newOutput(`out-${i}`, `device-${i}`))
  assert.equal(mix.outputs.length, 52)
})

test('missing sources never silently change the conservative bound', () => {
  const preview = createPreview()
  const mix = preview.mixes[1]!
  const before = peakBound(mix)
  preview.micMissing = true
  assert.equal(peakBound(mix), before)
  mix.sends['fixture:spotify']!.gain = 1
  assert.ok(peakBound(mix) > 1)
  assert.equal(mix.sends['fixture:mic']!.gain, 0.5)
})

test('mic monitoring consent follows the output route, independent of mix name', () => {
  const mix = newMix('arbitrary-id', 'Record')
  mix.sends['fixture:mic']!.gain = 1
  const physical = newOutput('output-a', 'fixture:speakers')
  const virtual = newOutput('output-b', 'fixture:record-input')
  assert.equal(needsMonitorConsent(mix, physical), true)
  assert.equal(needsMonitorConsent(mix, virtual), false)
  mix.name = 'Call'
  assert.equal(needsMonitorConsent(mix, physical), true)
  updateSend('demo', mix, 'fixture:mic', { muted: true })
  mix.master = 0
  physical.muted = true
  physical.gain = 0
  assert.equal(needsMonitorConsent(mix, physical), true)
  updateSend('demo', mix, 'fixture:mic', { gain: 0 })
  assert.equal(needsMonitorConsent(mix, physical), true)
  physical.monitorConsent = true
  assert.equal(needsMonitorConsent(mix, physical), false)
  assert.equal(needsMonitorConsent(mix, newOutput('other-output', 'fixture:headphones')), true)
})

test('primary pause on A affects only its associated player globally, preserves Play association', () => {
  const preview = createPreview()
  const a = newMix('music-a', 'Music A')
  const b = newMix('video-b', 'Video B')
  a.sends['fixture:spotify']!.gain = 1
  a.outputs.push(newOutput('a', 'fixture:headphones'))
  b.sends['fixture:vlc']!.gain = 1
  b.outputs.push(newOutput('b', 'fixture:speakers'))
  preview.mixes = [a, b]
  preview.controls['fixture:headphones']!.scope = 'primary'
  assert.deepEqual(routedPlayers(preview, 'fixture:headphones').map(player => player.name), ['Spotify'])
  assert.deepEqual(previewPlayback('demo', preview, 'fixture:headphones', 'paused'), ['Spotify'])
  assert.equal(preview.players[1]!.playback, 'playing')
  assert.equal(controlPlayers(preview, 'fixture:headphones')[0]!.name, 'Spotify')
  a.outputs.push(newOutput('a-on-b', 'fixture:speakers'))
  assert.equal(routedPlayers(preview, 'fixture:speakers').find(player => player.name === 'Spotify')!.playback, 'paused')
  previewPlayback('demo', preview, 'fixture:headphones', 'playing')
  assert.equal(preview.players[0]!.playback, 'playing')
})

test('primary requires its device route, retains paused association, and never falls back', () => {
  const preview = createPreview()
  const deviceId = 'fixture:headphones'
  const policy = preview.controls[deviceId]!
  policy.scope = 'primary'
  const monitor = preview.mixes[2]!
  const output = monitor.outputs.find(output => output.deviceId === deviceId)!
  monitor.outputs = monitor.outputs.filter(item => item.id !== output.id)
  assert.deepEqual(routedPlayers(preview, deviceId), [])
  assert.deepEqual(controlPlayers(preview, deviceId), [])
  assert.deepEqual(previewPlayback('demo', preview, deviceId, 'paused'), [])
  assert.deepEqual(routedPlayers(preview, 'fixture:speakers').map(player => player.playback), ['playing', 'playing'])
  assert.equal(policy.primaryPlayerId, 'fixture:spotify-session')

  monitor.outputs.push(output)
  assert.deepEqual(previewPlayback('demo', preview, deviceId, 'paused'), ['Spotify'])
  monitor.outputs = monitor.outputs.filter(item => item.id !== output.id)
  assert.deepEqual(previewPlayback('demo', preview, deviceId, 'playing'), [])
  assert.equal(preview.players[0]!.playback, 'paused')
  monitor.outputs.push(output)
  assert.deepEqual(controlPlayers(preview, deviceId).map(player => player.name), ['Spotify'])
  assert.deepEqual(previewPlayback('demo', preview, deviceId, 'playing'), ['Spotify'])

  monitor.sends['fixture:spotify']!.gain = 0
  assert.deepEqual(routedPlayers(preview, deviceId).map(player => player.name), ['VLC'])
  assert.deepEqual(previewPlayback('demo', preview, deviceId, 'paused'), [])
  assert.equal(preview.players[1]!.playback, 'playing')
})

test('unassigned devices and unknown primaries cannot control other players', () => {
  const preview = createPreview()
  const headset = preview.controls['fixture:bluetooth']!
  headset.scope = 'primary'
  assert.deepEqual(controlPlayers(preview, 'fixture:bluetooth'), [])
  assert.deepEqual(previewPlayback('demo', preview, 'fixture:bluetooth', 'paused'), [])
  preview.controls['fixture:headphones'] = { scope: 'primary', primaryPlayerId: 'unknown-session' }
  assert.equal(routedPlayers(preview, 'fixture:headphones').length, 2)
  assert.deepEqual(controlPlayers(preview, 'fixture:headphones'), [])
  assert.deepEqual(previewPlayback('demo', preview, 'fixture:headphones', 'paused'), [])
  assert.deepEqual(preview.players.map(player => player.playback), ['playing', 'playing'])
})

test('group actions set coherent explicit state; normal/disconnected modes dispatch nothing', () => {
  const preview = createPreview()
  const deviceId = 'fixture:headphones'
  assert.deepEqual(previewPlayback('demo', preview, deviceId, 'paused'), [])
  preview.controls[deviceId]!.scope = 'group'
  preview.players[0]!.playback = 'paused'
  previewPlayback('demo', preview, deviceId, 'paused')
  assert.deepEqual(preview.players.map(player => player.playback), ['paused', 'paused'])
  previewPlayback('disconnected', preview, deviceId, 'playing')
  assert.deepEqual(preview.players.map(player => player.playback), ['paused', 'paused'])
  previewPlayback('demo', preview, deviceId, 'playing')
  assert.deepEqual(preview.players.map(player => player.playback), ['playing', 'playing'])
})
