// Fixtures are UI previews, not discovery envelopes or runtime configuration.
export type Mode = 'disconnected' | 'demo'
export type SourceId = string
export type Send = { gain: number; muted: boolean; included: boolean }
export type Output = { id: string; name: string; deviceId: string; gain: number; muted: boolean; monitorConsent: boolean }
export type Mix = {
  id: string
  name: string
  master: number
  excludeCallReturn: boolean
  sends: Record<SourceId, Send>
  outputs: Output[]
}
export type ControlPolicy = { scope: 'normal' | 'primary' | 'group'; primaryPlayerId: string }
export type Preview = {
  mixes: Mix[]
  micMissing: boolean
  players: { id: string; name: string; sourceId: string; playback: 'playing' | 'paused' }[]
  controls: Record<string, ControlPolicy>
}

export const sources = [
  { id: 'fixture:mic', name: 'Studio mic', kind: 'microphone', detail: 'Mono · fixture', peak: -18 },
  { id: 'fixture:spotify', name: 'Spotify', kind: 'player', detail: 'Stereo · fixture', peak: -24 },
  { id: 'fixture:vlc', name: 'VLC', kind: 'player', detail: 'Stereo · fixture', peak: -21 },
  { id: 'fixture:return', name: 'Call return', kind: 'call-return', detail: 'Missing · fixture', peak: null },
] as const

export const devices = [
  { id: 'fixture:headphones', name: 'Headphones A', kind: 'physical', detail: 'Wired · fixture' },
  { id: 'fixture:speakers', name: 'Speakers B', kind: 'physical', detail: 'Wired · fixture' },
  { id: 'fixture:bluetooth', name: 'BT headset C', kind: 'physical', detail: 'Profile / codec unknown' },
  { id: 'fixture:call-input', name: 'Call app input', kind: 'virtual', detail: 'Transport unverified · fixture', excludeSourceId: 'fixture:return' },
  { id: 'fixture:record-input', name: 'Record app input', kind: 'virtual', detail: 'Transport unverified · fixture' },
] as const

export function newMix(id: string, name: string): Mix {
  return { id, name, master: 1, excludeCallReturn: false,
    sends: Object.fromEntries(sources.map(source => [source.id, { gain: 0, muted: false, included: false }])), outputs: [] }
}

export function newOutput(id: string, deviceId: string): Output {
  return { id, name: devices.find(device => device.id === deviceId)?.name ?? 'Output', deviceId, gain: 1, muted: false, monitorConsent: false }
}

export function createPreview(): Preview {
  const call = newMix('fixture:call', 'Call')
  call.excludeCallReturn = true
  call.master = 0.8
  call.sends['fixture:mic']!.gain = 1
  call.sends['fixture:spotify']!.gain = 0.2
  call.outputs.push(newOutput('fixture:call-output', 'fixture:call-input'))
  const record = newMix('fixture:record', 'Record')
  record.master = 0.8
  record.sends['fixture:mic']!.gain = 0.5
  record.sends['fixture:spotify']!.gain = 0.35
  record.sends['fixture:vlc']!.gain = 0.1
  record.sends['fixture:return']!.gain = 0.15
  record.outputs.push(newOutput('fixture:record-output', 'fixture:record-input'))
  const monitor = newMix('fixture:monitor', 'Monitor')
  monitor.master = 0.8
  monitor.sends['fixture:spotify']!.gain = 0.5
  monitor.sends['fixture:vlc']!.gain = 0.25
  monitor.sends['fixture:return']!.gain = 0.4
  monitor.outputs.push(newOutput('fixture:monitor-a', 'fixture:headphones'), newOutput('fixture:monitor-b', 'fixture:speakers'))
  for (const mix of [call, record, monitor]) {
    for (const send of Object.values(mix.sends)) send.included = send.gain > 0
  }
  return {
    mixes: [call, record, monitor], micMissing: false,
    players: [
      { id: 'fixture:spotify-session', name: 'Spotify', sourceId: 'fixture:spotify', playback: 'playing' },
      { id: 'fixture:vlc-session', name: 'VLC', sourceId: 'fixture:vlc', playback: 'playing' },
    ],
    controls: Object.fromEntries(devices.filter(device => device.kind === 'physical').map(device =>
      [device.id, { scope: 'normal', primaryPlayerId: 'fixture:spotify-session' }])),
  }
}

export function blockedSend(mix: Mix, sourceId: string): boolean {
  return (mix.excludeCallReturn && sources.some(source => source.id === sourceId && source.kind === 'call-return')) ||
    mix.outputs.some(output => devices.some(device => device.id === output.deviceId &&
      'excludeSourceId' in device && device.excludeSourceId === sourceId))
}

export function updateSend(mode: Mode, mix: Mix, sourceId: string, patch: Partial<Send>): boolean {
  const send = mix.sends[sourceId]
  if (mode !== 'demo' || !send || blockedSend(mix, sourceId)) return false
  if (patch.gain !== undefined && (!Number.isFinite(patch.gain) || patch.gain < 0 || patch.gain > 1)) return false
  Object.assign(send, patch)
  if (send.gain > 0) send.included = true
  return true
}

export function peakBound(mix: Mix, output?: Output): number {
  if (output?.muted) return 0
  return mix.master * (output?.gain ?? 1) * Object.entries(mix.sends).reduce((sum, [id, send]) =>
    sum + (send.muted || blockedSend(mix, id) ? 0 : send.gain), 0)
}

export function needsMonitorConsent(mix: Mix, output: Output): boolean {
  const mic = mix.sends['fixture:mic']
  return devices.some(device => device.id === output.deviceId && device.kind === 'physical') &&
    !!mic && (mic.included || mic.gain > 0) && !output.monitorConsent
}

// Associate sessions by configured routes, including paused players. Stream disappearance
// must not erase the player that a later Play action should address.
export function routedPlayers(preview: Preview, deviceId: string) {
  return preview.players.filter(player => preview.mixes.some(mix => {
    const send = mix.sends[player.sourceId]
    return mix.master > 0 && send && send.gain > 0 && !send.muted && mix.outputs.some(output =>
      output.deviceId === deviceId && output.gain > 0 && !output.muted)
  }))
}

export function controlPlayers(preview: Preview, deviceId: string) {
  const policy = preview.controls[deviceId]
  if (!policy || policy.scope === 'normal') return []
  return policy.scope === 'primary'
    ? routedPlayers(preview, deviceId).filter(player => player.id === policy.primaryPlayerId)
    : routedPlayers(preview, deviceId)
}

export function previewPlayback(mode: Mode, preview: Preview, deviceId: string, operation: 'playing' | 'paused'): string[] {
  if (mode !== 'demo') return []
  const players = controlPlayers(preview, deviceId)
  for (const player of players) player.playback = operation
  return players.map(player => player.name)
}

export function gainDb(gain: number): string {
  return gain > 0 ? `${(20 * Math.log10(gain)).toFixed(1)} dB` : '−∞ dB'
}
