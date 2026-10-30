import type { Invoke } from './transport.ts'

export async function closeInterface(command: 'hide_mixer' | 'quit_ui', platform: string | null,
  hasUnsavedDrafts: () => boolean, confirm: (message: string) => boolean, invoke: Invoke): Promise<boolean> {
  const exits = command === 'quit_ui' || platform !== 'macos'
  if (exits && hasUnsavedDrafts() && !confirm('Close Mixyard? Unsaved drafts on one or more devices will be lost. Cancel to save or export them first.')) return false
  await invoke(command)
  return true
}
