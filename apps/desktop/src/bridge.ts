export type ShellStatus = { schemaVersion: 1; connected: false; trayCreated: boolean; platform: string }
export const inDesktopShell = '__TAURI_INTERNALS__' in window

export async function shellStatus(): Promise<ShellStatus | null> {
  if (!inDesktopShell) return null
  const { invoke } = await import('@tauri-apps/api/core')
  const status = await invoke<ShellStatus>('shell_status')
  if (status.schemaVersion !== 1 || status.connected !== false) throw new Error('Unsupported shell status; controls remain disconnected.')
  return status
}

export async function shellAction(action: 'hide_mixer' | 'quit_ui'): Promise<void> {
  if (!inDesktopShell) throw new Error('Window actions require the desktop shell.')
  const { invoke } = await import('@tauri-apps/api/core')
  await invoke(action)
}
