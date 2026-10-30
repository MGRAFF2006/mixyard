import { readConnection } from './runtime.ts'
import type { ConnectionStatus } from './runtime.ts'
import type { Invoke } from './transport.ts'

export type ShellStatus = { schemaVersion: 1; connected: boolean; runtime: ConnectionStatus; trayCreated: boolean; platform: string; compact: boolean }
export type SessionSummary = { deviceId: string | null; authorityId: string | null; revision: number | null; displayName: string; connected: boolean; epoch: number }
export const inDesktopShell = '__TAURI_INTERNALS__' in window
export const nativeInvoke: Invoke = async <T>(command: string, args?: Record<string, unknown>): Promise<T> => {
  if (!inDesktopShell) throw new Error('This action requires the native desktop shell.')
  const bridge = (window as unknown as { __TAURI_INTERNALS__: { invoke: Invoke } }).__TAURI_INTERNALS__
  return bridge.invoke<T>(command, args)
}

export async function shellStatus(): Promise<ShellStatus | null> {
  if (!inDesktopShell) return null
  const status = await nativeInvoke<ShellStatus>('shell_status')
  if (status.schemaVersion !== 1 || typeof status.trayCreated !== 'boolean' || typeof status.platform !== 'string') throw new Error('Unsupported shell status.')
  status.runtime = readConnection(status.runtime)
  if (status.connected !== (status.runtime.phase === 'connected')) throw new Error('Inconsistent runtime connection state.')
  return status
}
export async function runtimeAction(action: 'connect_runtime' | 'disconnect_runtime', instance?: string): Promise<ConnectionStatus> {
  return readConnection(await nativeInvoke(action, action === 'connect_runtime' ? { instance } : {}))
}
export async function shellAction(action: 'hide_mixer' | 'quit_ui' | 'set_compact', compact?: boolean): Promise<void> {
  await nativeInvoke(action, action === 'set_compact' ? { compact } : undefined)
}
