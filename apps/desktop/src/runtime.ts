// Lifecycle data never enables the fixture model or any audio controls.
export type Availability = {
  daemonReachable: true; backendAvailable: false; backend: 'unavailable'
  audioRevision: null; appliedRevision: null; desiredRevision: null
  nativeAudioProof: 'unverified'
  capabilities: { apply: false; mute: false; nativeActionDispatch: false }
}
export type RuntimeEvent =
  | { event: 'snapshot'; sequence: 1; resync: true; lifecycle: 'running'; availability: Availability; historyAvailable: false }
  | { event: 'lifecycle'; sequence: 2; lifecycle: 'stopping'; backendAvailable: false }
export type ConnectionStatus = {
  phase: 'disconnected' | 'connecting' | 'connected' | 'stopped'
  instance: string | null; event: RuntimeEvent | null; error: string | null
}
export const disconnected = (): ConnectionStatus => ({ phase: 'disconnected', instance: null, event: null, error: null })

export function readConnection(value: unknown): ConnectionStatus {
  const status = value as ConnectionStatus | null
  if (!status || !['disconnected', 'connecting', 'connected', 'stopped'].includes(status.phase)
    || !(status.instance === null || typeof status.instance === 'string')
    || !(status.error === null || typeof status.error === 'string')) throw new Error('Unsupported runtime connection state.')
  if (status.phase === 'connected') {
    const event = status.event
    const a = event?.event === 'snapshot' ? event.availability : null
    if (!event || event.event !== 'snapshot' || event.sequence !== 1 || event.resync !== true
      || event.lifecycle !== 'running' || event.historyAvailable !== false || !a
      || a.daemonReachable !== true || a.backendAvailable !== false || a.backend !== 'unavailable'
      || a.audioRevision !== null || a.appliedRevision !== null || a.desiredRevision !== null
      || a.nativeAudioProof !== 'unverified' || a.capabilities?.apply !== false
      || a.capabilities?.mute !== false || a.capabilities?.nativeActionDispatch !== false)
      throw new Error('Runtime capability mismatch. Audio controls remain locked.')
  } else if (status.phase === 'stopped') {
    const e = status.event
    if (!e || e.event !== 'lifecycle' || e.sequence !== 2 || e.lifecycle !== 'stopping' || e.backendAvailable !== false)
      throw new Error('Unsupported runtime lifecycle event.')
  } else if (status.event !== null) throw new Error('Disconnected runtime state contains live data.')
  if ((status.phase === 'connected' || status.phase === 'connecting' || status.phase === 'stopped') && !status.instance)
    throw new Error('Runtime instance is missing.')
  return status
}

export function connectionLabel(status: ConnectionStatus): string {
  return status.phase === 'connected' ? 'Connected · audio unavailable'
    : status.phase === 'connecting' ? 'Connecting…'
    : status.phase === 'stopped' ? 'Runtime stopping · not connected' : 'Not connected'
}
