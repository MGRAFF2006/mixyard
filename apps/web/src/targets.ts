import type { Api } from './api.ts'
import type { CodecProbe } from './network-codecs.ts'
export type DeviceTarget = { id: string; displayName: string; api: Api; codecHost?: CodecProbe; retired?: boolean }

/** A target ID owns one transport and cached workspace for this window's lifetime. */
export class DeviceTargets {
  private readonly known = new Map<string, DeviceTarget>()
  current: readonly DeviceTarget[] = []
  reconcile(incoming: readonly DeviceTarget[], hasDrafts: (id: string) => boolean): readonly DeviceTarget[] {
    if (!incoming.length || new Set(incoming.map(t => t.id)).size !== incoming.length || incoming.some(t => !t.id || !t.displayName)) throw Error('Device targets require unique IDs, display names and at least one target.')
    for (const target of incoming) {
      const old = this.known.get(target.id)
      if (old && old.api !== target.api) throw Error('A device target ID cannot be rebound to another API. Use a new target ID for a new native credential.')
    }
    const next = incoming.map(target => ({ ...target, retired: false }))
    const ids = new Set(incoming.map(target => target.id))
    for (const target of this.current) {
      if (ids.has(target.id)) continue
      if (!target.retired) target.api.disconnect('Saved target removed; old authority discarded. Export retained drafts before closing this workspace.')
      if (hasDrafts(target.id)) next.push({ ...target, retired: true })
    }
    for (const target of next) this.known.set(target.id, target)
    this.current = next
    return next
  }
}
