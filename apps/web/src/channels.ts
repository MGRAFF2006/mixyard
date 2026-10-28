import type { ChainConfig, EffectConfig, MatrixRoute } from './dsp.ts'
import { newEffect } from './dsp.ts'

function known(config: ChainConfig, id: string) {
  if (!config.format.channels.includes(id)) throw new Error('Choose an existing channel ID. IDs are exact, including spaces.')
}
export function renameChannel(config: ChainConfig, old: string, next: string) {
  known(config, old)
  if (old === next) return
  if (!next.trim() || config.format.channels.includes(next)) throw new Error('Channel ID must be nonblank and unique. The previous ID and references were kept.')
  // A rename is one explicit identity edit, including every reference in the chain.
  config.format.channels[config.format.channels.indexOf(old)] = next
  for (const item of config.effects) {
    if (item.channels) item.channels = item.channels.map(id => id === old ? next : id)
    if (item.effect.kind === 'channel-matrix') for (const route of item.effect.routes) {
      if (route.output === old) route.output = next
      for (const input of route.inputs) if (input.channel === old) input.channel = next
    }
  }
}
export function removeChannel(config: ChainConfig, id: string) {
  known(config, id)
  if (config.format.channels.length === 1) throw new Error('Keep at least one channel.')
  const references = config.effects.filter(item => item.channels?.includes(id) || item.effect.kind === 'channel-matrix')
  if (references.length) throw new Error(`Cannot remove ${JSON.stringify(id)}: referenced by ${references.map(item => JSON.stringify(item.name)).join(', ')}. Edit the selections or explicitly remove those effects first. No settings were dropped.`)
  config.format.channels.splice(config.format.channels.indexOf(id), 1)
}
export function addChannel(config: ChainConfig, id: string) {
  if (!id.trim() || config.format.channels.includes(id)) throw new Error('Enter a nonblank, unique channel ID. IDs are preserved exactly.')
  if (config.effects.some(item => item.effect.kind === 'channel-matrix')) throw new Error('Channel matrices cover the fixed format. Export and edit the complete format/routes together through JSON import, or explicitly remove the matrices before adding a channel. Existing routes were kept.')
  config.format.channels.push(id)
}
export function channelLevel(channel: string, db: number, inverted: boolean): EffectConfig {
  return { name: `Level / polarity ${JSON.stringify(channel)}`, enabled: true, channels: [channel], effect: { kind: 'gain', gain: 10 ** (db / 20), invertPolarity: inverted } }
}
export function balance(channels: string[], first: string, second: string, amount: number): EffectConfig {
  if (first === second || !channels.includes(first) || !channels.includes(second)) throw new Error('Choose two different existing channels for the pair.')
  if (!Number.isFinite(amount) || Math.abs(amount) > 100) throw new Error('Balance must be between -100 and 100.')
  return { name: `Balance ${JSON.stringify(first)} / ${JSON.stringify(second)} (${amount})`, enabled: true, effect: { kind: 'channel-matrix', routes: channels.map(id => ({ output: id, inputs: [{ channel: id, gain: id === first && amount > 0 ? 1 - amount / 100 : id === second && amount < 0 ? 1 + amount / 100 : 1 }] })) } }
}
export function routing(channels: string[], first: string, second: string, mode: 'swap' | 'copy'): EffectConfig {
  if (first === second || !channels.includes(first) || !channels.includes(second)) throw new Error('Choose two different existing channels for the pair.')
  const item = newEffect('channel-matrix', channels)
  item.name = `${mode === 'swap' ? 'Swap' : 'Copy'} ${JSON.stringify(first)} ${mode === 'swap' ? 'with' : 'to'} ${JSON.stringify(second)}`
  if (item.effect.kind === 'channel-matrix') for (const route of item.effect.routes) {
    if (route.output === second) route.inputs = [{ channel: first, gain: 1 }]
    else if (mode === 'swap' && route.output === first) route.inputs = [{ channel: second, gain: 1 }]
  }
  return item
}
export function alignment(channel: string, frames: number): EffectConfig {
  if (!Number.isSafeInteger(frames) || frames < 1) throw new Error('Alignment needs a positive whole sample count. Fractional delay is not implemented.')
  return { name: `Alignment ${JSON.stringify(channel)}`, enabled: true, channels: [channel], effect: { kind: 'delay', frames, wet: 1, dry: 0, feedback: 0 } }
}
export function routeMode(route: MatrixRoute): string {
  if (!route.inputs.length) return 'silence'
  if (route.inputs.length === 1 && route.inputs[0]!.gain === 1) return 'channel:' + route.inputs[0]!.channel
  return 'custom'
}
export function matrixSummary(routes: MatrixRoute[]): string {
  if (routes.every(route => routeMode(route) === 'channel:' + route.output)) return 'Identity routing'
  if (routes.every(route => routeMode(route) !== 'custom')) return 'Channel routing / silence'
  return 'Custom matrix — coefficients preserved'
}
export function quantizeDelay(milliseconds: number, rate: number, mode: 'exact' | 'floor' | 'nearest' | 'ceil'): number | null {
  const samples = milliseconds * rate / 1000
  if (!Number.isFinite(samples) || samples < 0) return null
  const result = mode === 'exact' ? samples : mode === 'floor' ? Math.floor(samples) : mode === 'ceil' ? Math.ceil(samples) : Math.round(samples)
  return Number.isSafeInteger(result) && result >= 1 ? result : null
}
