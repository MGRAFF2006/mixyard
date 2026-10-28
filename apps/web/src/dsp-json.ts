// JSON.stringify changes -0 to 0. DSP samples/parameters use the full finite
// Float64 JSON domain, including signed zero and integers beyond 2^53.
export function dspJson(value: unknown, pretty = false): string {
  const encode = (v: unknown, depth: number): string => {
    if (typeof v === 'number') {
      if (!Number.isFinite(v)) throw new Error('DSP JSON requires finite Float64 numbers.')
      return Object.is(v, -0) ? '-0' : JSON.stringify(v)
    }
    if (v === null || typeof v === 'string' || typeof v === 'boolean') return JSON.stringify(v)
    const pad = pretty ? '  '.repeat(depth + 1) : '', end = pretty ? '  '.repeat(depth) : ''
    const join = (parts: string[], open: string, close: string) => parts.length === 0 ? open + close : pretty ? `${open}\n${pad}${parts.join(`,\n${pad}`)}\n${end}${close}` : open + parts.join(',') + close
    if (Array.isArray(v)) return join(v.map(item => encode(item, depth + 1)), '[', ']')
    if (typeof v === 'object') return join(Object.entries(v).map(([key, item]) => `${JSON.stringify(key)}:${pretty ? ' ' : ''}${encode(item, depth + 1)}`), '{', '}')
    throw new Error('Expected a JSON value.')
  }
  return encode(value, 0)
}
export function copyDsp<T>(value: T): T { return JSON.parse(dspJson(value)) as T }
