import { Api } from '../../web/src/api.ts'
import type { Permission } from '../../web/src/api.ts'
import type { Invoke } from './transport.ts'

export type PairedTarget = { deviceId: string; displayName: string; endpoint: string; certificateSha256: string; credentialId: string; permissions: Permission[] }
export const nativeCredentialSentinel = 'native-owned-paired-credential'
type OpenIntent = { generation: number; serial: number }
type Reply = { status: number; body: string; generation?: number }

/** A sentinel requests native authority; it is never an HTTP credential. */
export function pairedApi(invoke: Invoke, target: PairedTarget): Api {
  let generation: number | null = null
  let lifecycle = 0
  let opening: OpenIntent | null = null
  function invalidate() {
    lifecycle++
    const expectedGeneration = generation, intent = opening
    generation = null; opening = null
    if (intent) void invoke('paired_cancel_open', { targetId: target.deviceId, intent }).catch(() => { /* Native intent cancellation is expected-ticket scoped. */ })
    if (expectedGeneration !== null) void invoke('paired_disconnect', { targetId: target.deviceId, expectedGeneration }).catch(() => { /* Native generations also reject stale queued requests. */ })
  }
  const fetcher: typeof fetch = async (input, init) => {
    if (input !== '/control/session' && input !== '/control/request') throw new Error('Unsupported native paired route.')
    const headers = new Headers(init?.headers)
    if (init?.method !== 'POST' || typeof init.body !== 'string' || headers.get('authorization') !== `Bearer ${nativeCredentialSentinel}` || headers.get('content-type') !== 'application/json' || [...headers.keys()].some(key => !['authorization', 'content-type'].includes(key))) throw new Error('Paired requests require native-owned authority and raw JSON.')
    init.signal?.throwIfAborted()
    const epoch = lifecycle
    const current = generation
    // A timeout/abort belonging to an obsolete exchange cannot own the
    // generation established by a later explicit reconnect.
    const abort = () => { if (epoch === lifecycle) invalidate() }
    init.signal?.addEventListener('abort', abort, { once: true })
    let reply: Reply, intent: OpenIntent | null = null
    try {
      if (input === '/control/session') {
        if (init.body !== '{}') throw new Error('Session discovery requires an empty object.')
        const prepared = await invoke<OpenIntent>('paired_prepare_open', { targetId: target.deviceId })
        // Preparation is harmless. Check logical ownership BEFORE a delayed
        // worker can attempt generation admission or any vault/network work.
        if (epoch !== lifecycle || init.signal?.aborted) throw new DOMException('Paired preparation cancelled', 'AbortError')
        if (!prepared || !Number.isSafeInteger(prepared.generation) || prepared.generation < 0 || !Number.isSafeInteger(prepared.serial) || prepared.serial < 1) throw new Error('Invalid native open intent.')
        intent = prepared; opening = intent
        reply = await invoke<Reply>('paired_open', { targetId: target.deviceId, credentialId: target.credentialId, intent })
        if (!Number.isSafeInteger(reply.generation) || reply.generation !== intent.generation + 1) throw new Error('Invalid native session generation.')
        if (epoch !== lifecycle || init.signal?.aborted) {
          await invoke('paired_cancel_open', { targetId: target.deviceId, intent })
          throw new DOMException('Paired session cancelled', 'AbortError')
        }
        generation = reply.generation!
      } else {
        if (current === null) throw new Error('Refresh this paired target before another operation.')
        reply = await invoke<Reply>('paired_request', { targetId: target.deviceId, generation: current, raw: init.body })
      }
      if (epoch !== lifecycle) throw new DOMException('Paired target changed', 'AbortError')
      init.signal?.throwIfAborted()
      return new Response(reply.body, { status: reply.status, headers: { 'Content-Type': 'application/json' } })
    } catch (error) {
      if (epoch === lifecycle) invalidate()
      if (error instanceof DOMException) throw error
      throw new TypeError(`Native paired transport: ${String(error)}`)
    } finally {
      if (opening === intent) opening = null
      init.signal?.removeEventListener('abort', abort)
    }
  }
  class NativeApi extends Api {
    override disconnect(reason = '') { invalidate(); super.disconnect(reason) }
  }
  // NativeApi retains the normal Api control lifecycle, including its host epoch.
  return new NativeApi(fetcher, { deviceId: target.deviceId, credential: nativeCredentialSentinel })
}
