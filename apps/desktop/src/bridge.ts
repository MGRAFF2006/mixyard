import { readConnection } from './runtime.ts'
import type { ConnectionStatus } from './runtime.ts'
import { readInspection } from './inspection.ts'
import type { SnapshotInput } from './inspection.ts'
export type ShellStatus = { schemaVersion: 1; connected: boolean; runtime: ConnectionStatus; trayCreated: boolean; platform: string }
export const inDesktopShell = '__TAURI_INTERNALS__' in window

async function nativeInvoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import('@tauri-apps/api/core')
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      invoke<T>(command, args),
      new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error('Desktop response timed out. Quit and reopen the UI if it remains unresponsive.')), 2000) }),
    ])
  } finally { clearTimeout(timer) }
}

export async function shellStatus(): Promise<ShellStatus | null> {
  if (!inDesktopShell) return null
  const status = await nativeInvoke<ShellStatus>('shell_status')
  if (status.schemaVersion !== 1 || typeof status.trayCreated !== 'boolean' || typeof status.platform !== 'string')
    throw new Error('Unsupported shell status.')
  status.runtime = readConnection(status.runtime)
  if (status.connected !== (status.runtime.phase === 'connected')) throw new Error('Inconsistent runtime connection state.')
  return status
}

export async function runtimeAction(action: 'connect_runtime' | 'disconnect_runtime', instance?: string): Promise<ConnectionStatus> {
  if (!inDesktopShell) throw new Error('Connect requires the desktop shell. Start the runtime separately with the CLI.')
  return readConnection(await nativeInvoke(action, action === 'connect_runtime' ? { instance } : {}))
}

export async function inspectSnapshot(input: SnapshotInput) {
  if (!inDesktopShell) throw new Error('Snapshot inspection requires the desktop Rust core. Browser preview has no native inspector; no fixture was substituted.')
  return readInspection(await nativeInvoke<unknown>('inspect_snapshot', { request: input }))
}

export async function shellAction(action: 'hide_mixer' | 'quit_ui'): Promise<void> {
  if (!inDesktopShell) throw new Error('Window actions require the desktop shell.')
  await nativeInvoke(action)
}
