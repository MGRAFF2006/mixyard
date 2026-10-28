import { copyDsp, dspJson } from './dsp-json.ts'
import { array, integer, object, string } from './model.ts'
import type { Raw } from './model.ts'
// These are the core format-1 structures, not a reduced editor representation.
// Shape readers retain every original property; the core rejects unsupported fields.
export type ValueType = 'bool' | 'number' | 'text' | 'text-set'
export type Value = Raw & {
  type: ValueType
  value: boolean | number | string | string[]
}
export type FactKey = Raw & {
  name: string
  subject?: string | null
}
export type Definition = Raw & {
  name: string
  valueType: ValueType
  scope: 'global' | 'entity'
  maxAgeMs: number
  evidencePolicy: 'any' | 'user-declaration'
}
export type Predicate = Raw & {
  op: string
  value?: Value | number | string
  values?: Value[] | string[]
  min?: number
  max?: number
}
export type Condition = (Raw & {
  kind: 'fact'
  key: FactKey
  predicate: Predicate
}) | (Raw & {
  kind: 'all'
  conditions: Condition[]
}) | (Raw & {
  kind: 'any'
  conditions: Condition[]
}) | (Raw & {
  kind: 'not'
  condition: Condition
})
export type Override = Raw & {
  scope: 'send' | 'output' | 'mix'
  source?: string
  mix?: string
  output?: string
  mute?: boolean | null
  gain?: number | null
}
export type Action = (Raw & {
  kind: 'no-change'
}) | (Raw & {
  kind: 'select'
  profileId?: string | null
  overrides: Override[]
})
export type Node = (Raw & {
  id: string
  kind: 'decision'
  condition: Condition
  onTrue: string
  onFalse: string
  onUnknown: string
}) | (Raw & {
  id: string
  kind: 'terminal'
  action: Action
})
export type Graph = Raw & {
  schemaVersion: 1
  id: string
  name: string
  revision: number
  root: string
  nodes: Node[]
}
export type Fact = Raw & {
  key: FactKey
  status: 'known' | 'unknown' | 'unsupported' | 'error' | 'ambiguous'
  value?: Value | null
  provider: string
  evidence: string
  observedAtMs?: number | null
  validForMs: number
  completeness: 'complete' | 'partial'
  reason?: string | null
}
export type Snapshot = Raw & {
  schemaVersion: 1
  revision: number
  epoch: string
  facts: Fact[]
}
export type Context = Raw & {
  configRevision: number
  storeRevision: number
  inventoryRevision: number
  manualRevision: number
}
export type SituationRequest = Raw & {
  graph: Graph
  catalog: Definition[]
  snapshot: Snapshot
  nowMs: number
  context: Context
  manualOverride: boolean
}
export type Truth = 'true' | 'false' | 'unknown'
// Fact trace details and the shared observation index use the core camelCase struct.
export type Trace = (Raw & {
  kind: 'decision'
  node: string
  result: Truth
  next: string
}) | (Raw & {
  kind: 'terminal'
  node: string
}) | (Raw & {
  kind: 'manual-override'
}) | (Raw & {
  kind: 'fact'
  node: string
  detail: Raw & {
    key: FactKey
    predicate: Predicate
    result: Truth
    observationIndex: number | null
    ageMs: number | null
    fresh: boolean
    unknownReason: string | null
  }
})
export type Evaluation = Raw & {
  schemaVersion: 1
  token: Raw & {
    graphId: string
    graphRevision: number
    snapshotRevision: number
    epoch: string
    evaluatedAtMs: number
    context: Context
  }
  terminal: string | null
  action: Action
  observations: Fact[]
  trace: Trace[]
  traceTruncated: boolean
  audioApplied: false
}
export type Provider = Raw & {
  provider: string
  status: 'complete' | 'partial' | 'unsupported' | 'error'
  reason: string | null
}
export type LiveContext = Raw & {
  snapshot: Snapshot
  nowMs: number
  catalog: Definition[]
  providers: Provider[]
  applicationScope?: string[]
}
export const branches = [['onTrue', 'true'], ['onFalse', 'false'], ['onUnknown', 'unknown']] as const
export const LIMITS = 'Format 1: 256 nodes, depth 32, 512 facts/catalog entries, 256 set/list items, 1024-byte texts, 4096 work units. Trace: 256 entries and a shared 256 KiB evidence budget. Inputs: 1 MiB. Rejected data is never silently cut.'
export const MAX_AUTOMATION_BYTES = 1048576
const types = ['bool', 'number', 'text', 'text-set'] as const
function choice<T extends string>(v: unknown, choices: readonly T[]): T {
  if (typeof v !== 'string' || !choices.includes(v as T))
    throw new Error(`Expected ${choices.join(', ')}.`)
  return v as T
}
function text(v: unknown) {
  const s = string(v)
  if (new TextEncoder().encode(s).length > 1024)
    throw new Error('Text exceeds 1024 bytes.')
  return s
}
function finite(v: unknown) {
  if (typeof v !== 'number' || !Number.isFinite(v))
    throw new Error('Expected a finite Float64 number.')
  return v
}
function bool(v: unknown) {
  if (typeof v !== 'boolean')
    throw new Error('Expected true or false.')
  return v
}
function list(v: unknown, max = 256) {
  const a = array(v)
  if (a.length > max)
    throw new Error(`List exceeds ${max} items; nothing was truncated.`)
  return a
}
function unique(values: string[], what: string) {
  if (new Set(values).size !== values.length)
    throw new Error(`Duplicate ${what} identity.`)
}
function strings(v: unknown) {
  const values = list(v).map(text)
  unique(values, 'set')
  return values
}
function readValue(v: unknown): Value {
  const value = object(v)
  switch (choice(value.type, types)) {
    case 'bool':
      bool(value.value)
      break
    case 'number':
      finite(value.value)
      break
    case 'text':
      text(value.value)
      break
    case 'text-set': strings(value.value)
  }
  return value as Value
}
function readKey(v: unknown) {
  const key = object(v)
  text(key.name)
  if (key.subject != null)
    text(key.subject)
  return key as FactKey
}
export function readCatalog(v: unknown): Definition[] {
  const a = list(v, 512).map(item => {
    const d = object(item)
    text(d.name)
    choice(d.valueType, types)
    choice(d.scope, ['global', 'entity'])
    integer(d.maxAgeMs)
    choice(d.evidencePolicy, ['any', 'user-declaration'])
    return d as Definition
  })
  unique(a.map(d => d.name), 'catalog')
  return a
}
export function operators(type: ValueType): string[] {
  return ['eq', 'ne', ...(type === 'number' ? ['lt', 'le', 'gt', 'ge', 'range'] : []), ...(type === 'text-set' ? ['contains', 'contains-all', 'contains-any', 'count'] : ['in']), 'present']
}
export function defaultValue(type: ValueType): Value {
  return { type, value: type === 'bool' ? true : type === 'number' ? 0 : type === 'text-set' ? [] : '' }
}
export function defaultPredicate(type: ValueType, op = type === 'text-set' ? 'contains' : 'eq'): Predicate {
  if (op === 'present')
    return { op }
  if (op === 'range' || op === 'count')
    return { op, min: 0, max: 1 }
  if (['contains-all', 'contains-any', 'in'].includes(op))
    return { op, values: [] }
  return { op, value: op === 'contains' ? '' : ['lt', 'le', 'gt', 'ge'].includes(op) ? 0 : defaultValue(type) }
}
export function defaultCondition(catalog: Definition[]): Condition {
  const d = catalog[0]
  return { kind: 'fact', key: { name: d?.name ?? 'device.connected', ...(d?.scope === 'entity' ? { subject: '' } : {}) }, predicate: defaultPredicate(d?.valueType ?? 'text-set') }
}
function readCondition(v: unknown, catalog: Definition[], depth: number, work: {
  n: number
}): Condition {
  if (++work.n > 4096 || depth > 32)
    throw new Error('Condition exceeds depth 32 or 4096 work units.')
  const c = object(v)
  switch (choice(c.kind, ['all', 'any', 'not', 'fact'])) {
    case 'all':
    case 'any':
      list(c.conditions).forEach(child => readCondition(child, catalog, depth + 1, work))
      break
    case 'not':
      readCondition(c.condition, catalog, depth + 1, work)
      break
    case 'fact': {
      const key = readKey(c.key), d = catalog.find(d => d.name === key.name)
      if (!d)
        throw new Error(`Unregistered fact: ${key.name}. Add its catalog definition in expert JSON.`)
      if ((d.scope === 'entity') !== (key.subject != null))
        throw new Error(`Fact ${key.name} needs ${d.scope === 'entity' ? 'an exact subject ID' : 'no subject'}.`)
      const p = object(c.predicate), op = choice(p.op, operators(d.valueType))
      const typed = (v: unknown) => {
        const value = readValue(v)
        if (value.type !== d.valueType)
          throw new Error(`Predicate value must be ${d.valueType}.`)
      }
      if (op === 'eq' || op === 'ne')
        typed(p.value)
      else if (op === 'in')
        list(p.values).forEach(typed)
      else if (op === 'contains')
        text(p.value)
      else if (op === 'contains-all' || op === 'contains-any')
        strings(p.values)
      else if (op === 'range' || op === 'count') {
        const min = finite(p.min), max = finite(p.max)
        if (min > max)
          throw new Error('Minimum exceeds maximum.')
        if (op === 'count') {
          integer(min)
          integer(max)
          if (max > 4294967295)
            throw new Error('Count exceeds u32.')
        }
      }
      else if (op !== 'present')
        finite(p.value)
    }
  }
  return c as Condition
}
export function readAction(v: unknown): Action {
  const a = object(v)
  if (choice(a.kind, ['no-change', 'select']) === 'select') {
    if (a.profileId != null)
      text(a.profileId)
    const overrides = list(a.overrides)
    if (a.profileId == null && !overrides.length)
      throw new Error('Select needs an exact profile ID or scoped reduction. Use no-change otherwise.')
    const ids: string[] = []
    overrides.forEach(item => {
      const o = object(item), scope = choice(o.scope, ['send', 'output', 'mix'])
      ids.push(dspJson([scope, scope === 'output' ? text(o.output) : text(o.mix), scope === 'send' ? text(o.source) : '']))
      if (o.mute != null && o.mute !== true)
        throw new Error('Overrides may only mute:true, never unmute.')
      if (o.gain != null && (finite(o.gain) < 0 || finite(o.gain) > 1))
        throw new Error('Attenuation gain must be between 0 and 1.')
      if (o.mute == null && o.gain == null)
        throw new Error('Override needs mute:true and/or attenuation gain.')
    })
    unique(ids, 'override')
  }
  return a as Action
}
export function graphIssues(graph: Graph): string[] {
  const issues: string[] = [], ids = new Set(graph.nodes.map(n => n.id))
  if (ids.size !== graph.nodes.length)
    issues.push('Duplicate node identity.')
  if (!ids.has(graph.root))
    issues.push(`Missing root: ${graph.root}`)
  const incoming = new Map(graph.nodes.map(n => [n.id, 0])), edges = new Map<string, string[]>()
  for (const n of graph.nodes) {
    const targets = n.kind === 'decision' ? branches.map(([key]) => n[key]) : []
    edges.set(n.id, targets)
    for (const target of targets) {
      if (!ids.has(target))
        issues.push(`${n.id}: missing branch target ${target}`)
      else
        incoming.set(target, incoming.get(target)! + 1)
    }
  }
  const queue = [...incoming].filter(([, degree]) => degree === 0).map(([id]) => id), depths = new Map(graph.nodes.map(n => [n.id, 1]))
  let visited = 0
  for (let i = 0; i < queue.length; i++) {
    const id = queue[i]!
    visited++
    for (const target of edges.get(id) ?? []) {
      if (!ids.has(target))
        continue
      depths.set(target, Math.max(depths.get(target)!, depths.get(id)! + 1))
      incoming.set(target, incoming.get(target)! - 1)
      if (incoming.get(target) === 0)
        queue.push(target)
    }
  }
  if (visited < ids.size)
    issues.push('Decision graph contains a cycle, including unreachable nodes.')
  if ([...depths.values()].some(d => d > 32))
    issues.push('Graph path exceeds 32 nodes.')
  return [...new Set(issues)]
}
export function readGraph(v: unknown, catalog: Definition[]): Graph {
  const g = object(v)
  if (g.schemaVersion !== 1)
    throw new Error('Expected graph schemaVersion 1.')
  text(g.id)
  text(g.name)
  integer(g.revision)
  text(g.root)
  const nodes = list(g.nodes)
  if (!nodes.length)
    throw new Error('Graph needs at least one node.')
  const work = { n: nodes.length + catalog.length }
  nodes.forEach(item => {
    const n = object(item)
    text(n.id)
    if (choice(n.kind, ['decision', 'terminal']) === 'terminal')
      readAction(n.action)
    else {
      readCondition(n.condition, catalog, 1, work)
      branches.forEach(([key]) => text(n[key]))
    }
  })
  const graph = g as Graph, issues = graphIssues(graph)
  if (issues.length)
    throw new Error(issues.join('\n'))
  return graph
}
export function readSnapshot(v: unknown, catalog: Definition[]): Snapshot {
  const s = object(v)
  if (s.schemaVersion !== 1)
    throw new Error('Expected snapshot schemaVersion 1.')
  integer(s.revision)
  text(s.epoch)
  const keys: string[] = []
  list(s.facts, 512).forEach(item => {
    const f = object(item), key = readKey(f.key), d = catalog.find(d => d.name === key.name)
    if (!d || (d.scope === 'entity') !== (key.subject != null))
      throw new Error(`Observation key does not match catalog: ${key.name}`)
    keys.push(dspJson([key.name, key.subject ?? null]))
    choice(f.status, ['known', 'unknown', 'unsupported', 'error', 'ambiguous'])
    text(f.provider)
    choice(f.evidence, ['platform-observed', 'user-declared', 'derived', 'fixture'])
    if (f.observedAtMs != null)
      integer(f.observedAtMs)
    integer(f.validForMs)
    choice(f.completeness, ['complete', 'partial'])
    if (f.reason != null)
      text(f.reason)
    if (f.status === 'known') {
      if (readValue(f.value).type !== d.valueType)
        throw new Error('Observation value type differs from catalog.')
    }
    else if (f.value != null)
      throw new Error('Non-known observations cannot have a value.')
  })
  unique(keys, 'observation')
  return s as Snapshot
}
function readContext(v: unknown): Context {
  const c = object(v)
  for (const key of ['configRevision', 'storeRevision', 'inventoryRevision', 'manualRevision'])
    integer(c[key])
  return c as Context
}
// Check raw extras too, without coercion or pruning. Avoid recursive serializer overflow.
function checkRaw(v: unknown) {
  const stack = [{ v, depth: 0 }]
  while (stack.length) {
    const { v, depth } = stack.pop()!
    if (depth > 128)
      throw new Error('JSON nesting exceeds supported import depth.')
    if (typeof v === 'number')
      finite(v)
    if (v && typeof v === 'object')
      for (const child of Object.values(v))
        stack.push({ v: child, depth: depth + 1 })
  }
}
export function readRequest(v: unknown): SituationRequest {
  checkRaw(v)
  const r = object(v), catalog = readCatalog(r.catalog)
  readGraph(r.graph, catalog)
  readSnapshot(r.snapshot, catalog)
  integer(r.nowMs)
  readContext(r.context)
  bool(r.manualOverride)
  if (new TextEncoder().encode(dspJson(r)).length > MAX_AUTOMATION_BYTES)
    throw new Error('Automation request exceeds 1 MiB.')
  return r as SituationRequest
}
// Keep integer metadata lexical: JSON.parse may round a fractional decimal
// into a safe integer before ordinary shape validation can inspect it.
export function metadataIntegerLiteral(value: string): boolean {
  return /^(0|[1-9][0-9]*)$/.test(value) && Number.isSafeInteger(Number(value))
}
const integerFields = new Set(['schemaVersion', 'revision', 'nowMs', 'configRevision', 'storeRevision', 'inventoryRevision', 'manualRevision', 'maxAgeMs', 'observedAtMs', 'validForMs'])
function checkIntegerLiterals(source: string) {
  let at = 0
  function whitespace() { while (/\s/.test(source[at] ?? '') && at < source.length) at++ }
  function visit(depth: number): void {
    if (depth > 128) throw new Error('JSON nesting exceeds supported import depth.')
    whitespace()
    if (source[at] === '{') {
      at++; whitespace()
      const numbers: [string, string][] = []
      let op = ''
      while (source[at] !== '}') {
        const start = at; stringToken(); const key = JSON.parse(source.slice(start, at)) as string
        whitespace(); at++; whitespace()
        const valueStart = at
        visit(depth + 1)
        const raw = source.slice(valueStart, at)
        if (key === 'op') op = JSON.parse(raw) as string
        if (/^-?[0-9]/.test(raw)) numbers.push([key, raw])
        whitespace()
        if (source[at] !== ',') break
        at++; whitespace()
      }
      at++
      for (const [key, raw] of numbers)
        if ((integerFields.has(key) || (op === 'count' && (key === 'min' || key === 'max'))) && !metadataIntegerLiteral(raw))
          throw new Error(`Enter an exact nonnegative integer literal for ${key}.`)
    }
    else if (source[at] === '[') {
      at++; whitespace()
      while (source[at] !== ']') { visit(depth + 1); whitespace(); if (source[at] !== ',') break; at++; whitespace() }
      at++
    }
    else if (source[at] === '"') stringToken()
    else { while (at < source.length && !/[\s,}\]]/.test(source[at]!)) at++ }
  }
  function stringToken() {
    at++
    while (at < source.length && source[at] !== '"') { if (source[at] === '\\') at++; at++ }
    at++
  }
  // Syntax is checked by JSON.parse before this bounded traversal.
  visit(0)
}
// JSON.parse accepts duplicate object keys by dropping earlier fields. Reject
// that ambiguous input before it can bypass the core's duplicate-field parser.
function rejectDuplicateFields(text: string) {
  const stack: (Set<string> | null)[] = []
  for (let i = 0; i < text.length; i++) {
    const c = text[i]
    if (c === '{') stack.push(new Set())
    else if (c === '[') stack.push(null)
    else if (c === '}' || c === ']') stack.pop()
    else if (c === '"') {
      const start = i++
      while (i < text.length && text[i] !== '"') { if (text[i] === '\\') i++; i++ }
      const end = i + 1
      let next = end
      while (/\s/.test(text[next] ?? '') && next < text.length) next++
      const fields = stack.at(-1)
      if (text[next] === ':' && fields) {
        const key = JSON.parse(text.slice(start, end)) as string
        if (fields.has(key)) throw new Error(`Duplicate JSON field: ${key}. No fields were dropped.`)
        fields.add(key)
      }
    }
  }
}
export function parseSituation(text: string): SituationRequest {
  if (new TextEncoder().encode(text).length > MAX_AUTOMATION_BYTES)
    throw new Error('Automation import exceeds 1 MiB.')
  const value: unknown = JSON.parse(text)
  rejectDuplicateFields(text)
  checkIntegerLiterals(text)
  return readRequest(value)
}

export function readLiveContext(v: unknown): LiveContext {
  const r = object(v)
  if (r.mode !== 'read-only-live-context')
    throw new Error('Unsupported live context mode.')
  const catalog = readCatalog(r.catalog), snapshot = readSnapshot(r.snapshot, catalog)
  integer(r.nowMs)
  const providers = array(r.providers).map(v => {
    const p = object(v)
    text(p.provider)
    choice(p.status, ['complete', 'partial', 'unsupported', 'error'])
    if (p.reason !== null)
      text(p.reason)
    return p as Provider
  })
  if (r.applicationScope !== undefined) strings(r.applicationScope)
  return { ...r, snapshot, catalog, nowMs: r.nowMs as number, providers } as LiveContext
}
export function readEvaluation(v: unknown, request: SituationRequest): Evaluation {
  const r = object(v)
  if (r.mode !== 'simulation')
    throw new Error('Unsupported evaluation mode.')
  const e = object(r.evaluation), token = object(e.token)
  if (e.schemaVersion !== 1 || e.audioApplied !== false)
    throw new Error('Unsupported simulation result; no audio application is allowed.')
  if (token.graphId !== request.graph.id || token.graphRevision !== request.graph.revision || token.snapshotRevision !== request.snapshot.revision || token.epoch !== request.snapshot.epoch || token.evaluatedAtMs !== request.nowMs || ['configRevision', 'storeRevision', 'inventoryRevision', 'manualRevision'].some(key => readContext(token.context)[key] !== request.context[key]))
    throw new Error('Evaluation token does not match the requested graph and context.')
  readAction(e.action)
  bool(e.traceTruncated)
  const nodes = new Map(request.graph.nodes.map(n => [n.id, n]))
  const terminal = e.terminal === null ? null : nodes.get(text(e.terminal))
  const normalizedAction = (value: unknown) => {
    const action = readAction(value)
    return action.kind === 'no-change' ? { kind: action.kind } : { kind: action.kind, profileId: action.profileId ?? null, overrides: action.overrides.map(o => ({ scope: o.scope, source: o.source ?? null, mix: o.mix ?? null, output: o.output ?? null, mute: o.mute ?? null, gain: o.gain ?? null })) }
  }
  if (request.manualOverride ? e.terminal !== null || object(e.action).kind !== 'no-change' : !terminal || terminal.kind !== 'terminal' || dspJson(normalizedAction(e.action)) !== dspJson(normalizedAction(terminal.action)))
    throw new Error('Evaluation terminal/action does not match the requested graph or manual override.')
  const observations = list(e.observations, 512)
  // Reuse observation shape checks; trace evidence can be a subset of supplied facts.
  readSnapshot({ schemaVersion: 1, revision: 0, epoch: request.snapshot.epoch, facts: observations }, request.catalog)
  let nextNode = request.graph.root
  if (request.manualOverride && observations.length) throw new Error('Manual override cannot include fact observations.')
  list(e.trace).forEach(v => {
    const t = object(v)
    if (request.manualOverride !== (t.kind === 'manual-override')) throw new Error('Trace contradicts manual override.')
    const node = t.kind === 'manual-override' ? null : nodes.get(text(t.node))
    if (node && node.id !== nextNode) throw new Error('Trace node is outside the evaluated path.')
    switch (choice(t.kind, ['decision', 'fact', 'terminal', 'manual-override'])) {
      case 'decision':
        text(t.node)
        text(t.next)
        choice(t.result, ['true', 'false', 'unknown'])
        if (!node || node.kind !== 'decision' || t.next !== node[t.result === 'true' ? 'onTrue' : t.result === 'false' ? 'onFalse' : 'onUnknown']) throw new Error('Trace branch does not match the graph.')
        nextNode = t.next as string
        break
      case 'fact':
        {
          if (!node || node.kind !== 'decision') throw new Error('Fact trace references a missing decision.')
          const d = object(t.detail)
          readKey(d.key)
          object(d.predicate)
          choice(d.result, ['true', 'false', 'unknown'])
          bool(d.fresh)
          if (d.ageMs !== null)
            integer(d.ageMs)
          if (d.unknownReason !== null)
            text(d.unknownReason)
          if (d.observationIndex !== null && integer(d.observationIndex) >= observations.length)
            throw new Error('Trace references missing evidence.')
          if (d.observationIndex !== null) {
            const key = object(object(observations[d.observationIndex as number]).key), detail = object(d.key)
            if (key.name !== detail.name || (key.subject ?? null) !== (detail.subject ?? null)) throw new Error('Trace observation key does not match its evidence.')
          }
          break
        }
      case 'terminal': if (!node || node.kind !== 'terminal' || node.id !== e.terminal) throw new Error('Trace terminal does not match the result.')
    }
  })
  return e as Evaluation
}
export function renameNode(graph: Graph, oldId: string, newId: string): Graph {
  text(newId)
  if (!graph.nodes.some(n => n.id === oldId))
    throw new Error('Node no longer exists.')
  if (oldId !== newId && graph.nodes.some(n => n.id === newId))
    throw new Error(`Node ID already exists: ${newId}`)
  const next = copyDsp(graph)
  next.nodes.forEach(n => {
    if (n.id === oldId)
      n.id = newId
    if (n.kind === 'decision')
      branches.forEach(([key]) => {
        if (n[key] === oldId)
          n[key] = newId
      })
  })
  if (next.root === oldId)
    next.root = newId
  return next
}
export function references(graph: Graph, id: string): string[] {
  return [...(graph.root === id ? ['graph root'] : []), ...graph.nodes.flatMap(n => n.kind === 'decision' ? branches.filter(([key]) => n[key] === id).map(([, truth]) => `${n.id} (${truth})`) : [])]
}
export function deleteNode(graph: Graph, id: string): Graph {
  const refs = references(graph, id)
  if (refs.length)
    throw new Error(`Relink before deleting ${id}: ${refs.join(', ')}`)
  return { ...graph, nodes: graph.nodes.filter(n => n.id !== id) }
}
export function nextNodeId(graph: Graph) {
  let i = 1
  while (graph.nodes.some(n => n.id === `node-${i}`))
    i++
  return `node-${i}`
}
export function conditionSummary(c: Condition): string {
  if (c.kind === 'not')
    return `Not (${conditionSummary(c.condition)})`
  if (c.kind === 'all' || c.kind === 'any')
    return `${c.kind === 'all' ? 'All' : 'Any'} of ${c.conditions.length} conditions`
  return `${c.key.name}${c.key.subject != null ? ` [${c.key.subject}]` : ''} ${c.predicate.op}`
}
export function actionSummary(a: Action): string {
  return a.kind === 'no-change' ? 'No change' : `${a.profileId != null ? `Select profile ${a.profileId}` : 'Scoped reductions'}${a.overrides.length ? `; ${a.overrides.length} reduction(s)` : ''}`
}
export function standardCatalog(): Definition[] {
  const entries: [
    string,
    ValueType,
    'global' | 'entity'
  ][] = [
    ['device.connected', 'text-set', 'global'], ['device.default-input', 'text', 'global'], ['device.default-output', 'text', 'global'], ['device.selected', 'text-set', 'global'], ['device.kind', 'text', 'entity'], ['device.capabilities', 'text-set', 'entity'], ['device.channels', 'number', 'entity'], ['device.sample-rate-hz', 'number', 'entity'], ['device.hardware-monitor-declared', 'bool', 'entity'],
    ['application.running', 'text-set', 'global'], ['application.foreground', 'text', 'global'], ['application.fullscreen', 'bool', 'entity'], ['application.playback', 'bool', 'entity'], ['application.capture', 'bool', 'entity'], ['application.call-active', 'bool', 'entity'], ['stream.active', 'text-set', 'global'], ['peer.connected', 'text-set', 'global'], ['network.connected', 'bool', 'global'], ['network.ssid', 'text', 'global'], ['power.source', 'text', 'global'], ['power.battery-percent', 'number', 'global'], ['host.identity', 'text', 'global'], ['civil.timezone', 'text', 'global'], ['civil.date', 'text', 'global'], ['civil.weekday', 'number', 'global'], ['civil.minute-of-day', 'number', 'global'], ['manual.flag', 'bool', 'entity'], ['profile.current', 'text', 'global'],
  ]
  return entries.map(([name, valueType, scope]) => ({ name, valueType, scope, maxAgeMs: 5000, evidencePolicy: name === 'device.hardware-monitor-declared' ? 'user-declaration' : 'any' }))
}
export function emptyRequest(): SituationRequest {
  return { graph: { schemaVersion: 1, id: 'situations', name: 'My situations', revision: 0, root: 'safe', nodes: [{ id: 'safe', kind: 'terminal', action: { kind: 'no-change' } }] }, catalog: standardCatalog(), snapshot: { schemaVersion: 1, revision: 0, epoch: 'simulation', facts: [] }, nowMs: 0, context: { configRevision: 0, storeRevision: 0, inventoryRevision: 0, manualRevision: 0 }, manualOverride: false }
}
export type Preset = {
  kind: 'headphones' | 'monitor' | 'applications'
  device: string
  endpointGroup: string[]
  profile: string
  normalProfile: string
  unknownProfile: string
  output: string
  source: string
  mix: string
  declared: boolean
  apps: string[]
  power: string
  timezone: string
  startMinute: number
  endMinute: number
  useTime: boolean
}
export function emptyPreset(): Preset {
  return { kind: 'headphones', device: '', endpointGroup: [], profile: '', normalProfile: '', unknownProfile: '', output: '', source: '', mix: '', declared: false, apps: [], power: '', timezone: '', startMinute: 540, endMinute: 1020, useTime: false }
}
export function monitorEndpoints(p: Preset): string[] {
  return p.endpointGroup.length ? [...p.endpointGroup] : [p.device]
}
export function presetProblems(p: Preset): string[] {
  const missing: string[] = []
  if (p.kind !== 'applications' && !p.device.trim())
    missing.push(p.kind === 'monitor' ? 'Hardware-monitor declaration endpoint UID' : 'Exact device UID')
  if (p.kind === 'headphones') {
    if (!p.profile.trim())
      missing.push('Headphone profile ID')
    if (!p.normalProfile.trim())
      missing.push('Normal profile ID')
    if (!p.output.trim())
      missing.push('Speaker output ID')
  }
  if (p.kind === 'monitor') {
    const endpoints = monitorEndpoints(p)
    if (endpoints.length > 256 || endpoints.some(id => !id.trim()) || new Set(endpoints).size !== endpoints.length) missing.push('1–256 unique, nonblank endpoint group IDs')
    if (!endpoints.includes(p.device)) missing.push('Declaration endpoint UID must be a listed group member')
    if (!p.declared)
      missing.push('Explicit hardware-monitor declaration')
    if (!p.source.trim())
      missing.push('Mic source ID')
    if (!p.mix.trim())
      missing.push('Monitor mix ID')
  }
  if (p.kind === 'applications') {
    if (!p.apps.length || p.apps.some(a => !a.trim()))
      missing.push('Exact application IDs')
    if (!p.profile.trim())
      missing.push('Active profile ID')
    if (!p.normalProfile.trim())
      missing.push('Normal profile ID')
  }
  if (p.useTime && (!p.timezone.trim() || !Number.isSafeInteger(p.startMinute) || !Number.isSafeInteger(p.endMinute) || p.startMinute < 0 || p.endMinute > 1439 || p.startMinute > p.endMinute))
    missing.push('Explicit IANA timezone and inclusive minute range 0–1439 (split overnight ranges using Any)')
  if (p.unknownProfile && !p.unknownProfile.trim()) missing.push('Unknown branch profile ID must be nonblank or empty')
  if (p.power && !p.power.trim()) missing.push('Power source must be nonblank or empty')
  return missing
}
export function buildPreset(p: Preset, base: SituationRequest): SituationRequest {
  const missing = presetProblems(p)
  if (missing.length)
    throw new Error(`Preset incomplete: ${missing.join('; ')}.`)
  const r = copyDsp(base)
  const fact = (name: string, predicate: Predicate, subject?: string): Condition => ({ kind: 'fact', key: { name, ...(subject === undefined ? {} : { subject }) }, predicate })
  const endpoints = monitorEndpoints(p)
  const conditions: Condition[] = p.kind === 'applications' ? [fact('application.running', { op: 'contains-all', values: p.apps })] : p.kind === 'monitor' ? [fact('device.connected', { op: 'contains-all', values: endpoints })] : [fact('device.connected', { op: 'contains', value: p.device })]
  if (p.kind === 'monitor') {
    conditions.push(fact('device.connected', { op: 'count', min: endpoints.length, max: endpoints.length }))
    conditions.push(fact('device.hardware-monitor-declared', { op: 'eq', value: { type: 'bool', value: true } }, p.device))
  }
  if (p.power)
    conditions.push(fact('power.source', { op: 'eq', value: { type: 'text', value: p.power } }))
  if (p.useTime) {
    conditions.push(fact('civil.timezone', { op: 'eq', value: { type: 'text', value: p.timezone } }))
    conditions.push(fact('civil.minute-of-day', { op: 'range', min: p.startMinute, max: p.endMinute }))
  }
  const active: Action = p.kind === 'monitor' ? { kind: 'select', overrides: [{ scope: 'send', source: p.source, mix: p.mix, mute: true }] } : { kind: 'select', profileId: p.profile, overrides: p.kind === 'headphones' ? [{ scope: 'output', output: p.output, mute: true }] : [] }
  r.graph = { ...r.graph, root: 'when', nodes: [{ id: 'when', kind: 'decision', condition: conditions.length === 1 ? conditions[0]! : { kind: 'all', conditions }, onTrue: 'active', onFalse: 'normal', onUnknown: 'safe' }, { id: 'active', kind: 'terminal', action: active }, { id: 'normal', kind: 'terminal', action: p.kind === 'monitor' ? { kind: 'no-change' } : { kind: 'select', profileId: p.normalProfile, overrides: [] } }, { id: 'safe', kind: 'terminal', action: p.unknownProfile ? { kind: 'select', profileId: p.unknownProfile, overrides: [] } : { kind: 'no-change' } }] }
  // A declaration is a simulation claim, never a fabricated platform observation.
  if (p.kind === 'monitor') {
    r.snapshot.facts = r.snapshot.facts.filter(f => f.key.name !== 'device.hardware-monitor-declared' || f.key.subject !== p.device)
    r.snapshot.facts.push({ key: { name: 'device.hardware-monitor-declared', subject: p.device }, status: 'known', value: { type: 'bool', value: true }, provider: 'user-simulation', evidence: 'user-declared', observedAtMs: r.nowMs, validForMs: 5000, completeness: 'complete', reason: 'Explicit declaration supplied for simulation' })
  }
  return r
}
