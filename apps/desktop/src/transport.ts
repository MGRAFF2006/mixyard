export type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>

/** The native boundary accepts local console routes only, never arbitrary fetch. */
export function desktopFetcher(invoke: Invoke): typeof fetch {
  return async (input, init) => {
    if (typeof input !== 'string' || !input.startsWith('/api/'))
      throw new Error('Remote control is unsupported by this local desktop transport. Use an explicitly paired TLS client.')
    const headers = new Headers(init?.headers)
    for (const key of headers.keys()) {
      if (key !== 'content-type' && key !== 'x-mixyard-token') throw new Error(`Desktop transport does not forward ${key} headers.`)
    }
    if (init?.body !== undefined && typeof init.body !== 'string') throw new Error('Desktop console requires a JSON string body.')
    init?.signal?.throwIfAborted()
    let reply: { status: number; body: string }
    try {
      reply = await invoke<typeof reply>('console_request', { request: {
        path: input, method: init?.method ?? 'GET', token: headers.get('x-mixyard-token'), body: init?.body ?? null,
      } })
    } catch (cause) {
      // Tauri rejects Result::Err with a string, while fetch reports network
      // failure as TypeError. Preserve the shared Api's invalidation contract;
      // canonical HTTP errors remain ordinary Response values below.
      init?.signal?.throwIfAborted()
      throw new TypeError('Local console transport failed.', { cause })
    }
    // An admitted write may finish after cancellation. Never replay or publish its completion.
    init?.signal?.throwIfAborted()
    return new Response(reply.body, { status: reply.status, headers: { 'Content-Type': 'application/json' } })
  }
}
