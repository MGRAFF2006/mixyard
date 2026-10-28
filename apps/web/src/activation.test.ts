import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Api, ApiError } from './api.ts'
import { ActivationSession } from './activation-session.ts'
import { parseSettings, readActivationStatus, readLive, readSettingsSnapshot, sameJson } from './activation.ts'
import type { SettingsSnapshot, Status } from './activation.ts'
import { copyDsp, dspJson } from './dsp-json.ts'
import { liveFixture, savedSettings, status } from '../tests/activation-fixture.ts'

test('complete settings preserve exact graph/catalog, raw parameters, Float64 and signed zero; metadata and duplicate maps reject', () => {
  const saved = savedSettings(), graph = saved.settings.graph!
  graph.raw = { signed: -0, largeFloat: 9007199254740994, low: 1.0000000000000002 }
  saved.settings.observer.raw = 'kept until authoritative validation'
  saved.settings.declarations = [{ key: { name: 'device.hardware-monitor-declared', subject: ' Exact/Ω ' }, value: false }]
  saved.settings.observer.applications = { '/opt/My App/real': 'Exact/App' }
  const reread = parseSettings(dspJson(saved))
  assert.deepEqual(reread, saved)
  assert.ok(Object.is((reread.settings.graph!.raw as { signed: number }).signed, -0))
  assert.equal(sameJson({ one: -0, two: [1, 2] }, { two: [1, 2], one: -0 }), true)
  assert.equal(sameJson({ one: -0 }, { one: 0 }), false)
  for (const [field, invalid] of [['settingsRevision', '4.0000000000000001'], ['settleMs', '1e3'], ['cooldownMs', '2.00000000000000001']]) {
    assert.throws(() => parseSettings(dspJson(saved).replace(new RegExp(`"${field}":[0-9]+`), `"${field}":${invalid}`)), /exact nonnegative integer literal/)
  }
  assert.throws(() => parseSettings(dspJson(saved).replace('"/opt/My App/real":"Exact/App"', '"/opt/My App/real":"one","/opt/My App/real":"two"')), /Duplicate JSON field/)
  const bad = copyDsp(saved); bad.settings.transitionPolicy.cooldownMs = 86400001; assert.throws(() => readSettingsSnapshot(bad), /Duration/)
  bad.settings.transitionPolicy.cooldownMs = 0; bad.settings.observer.applications = { relative: 'app' }; assert.throws(() => readSettingsSnapshot(bad), /absolute/)
})

test('live reader checks audio authority, complete DTOs, token/capture/revision pairing and every reachable Select candidate', () => {
  const saved = savedSettings(), good = liveFixture(saved)
  assert.equal(readLive(good, saved, true).review!.actions.length, 2)
  const cases: ((v: ReturnType<typeof liveFixture>) => void)[] = [
    v => { (v as Record<string, unknown>).audioApplied = true },
    v => { (v.status as Record<string, unknown>).backendAvailable = true },
    v => { v.status.manualRevision = 0.5 },
    v => { v.request!.manualOverride = true },
    v => { v.request!.nowMs++ },
    v => { v.context!.snapshot.epoch = 'wrong' },
    v => { v.inventory!.revision++ },
    v => { v.review!.authority.storeRevision++ },
    v => { v.review!.actions.pop() },
    v => { v.review!.actions.push(v.review!.actions[0]!) },
    v => { v.review!.actions[0]!.candidate.revision = 10 },
    v => { delete (v.review!.actions[0]!.preview.changes as Record<string, unknown>).mediaControls },
    v => { delete v.review!.actions[0]!.preview.plan.diagnostics },
    v => { v.review!.actions[0]!.preview.plan.sources = [] },
    v => { v.review!.actions[0]!.preview.unavailableIdentities.outputs = [] },
    v => { v.review!.actions[0]!.preview.sensitivePaths[0]!.target = {} },
    v => { v.preview = v.review!.actions[0]!; v.inventory = null },
  ]
  for (const change of cases) { const bad = copyDsp(good); change(bad); assert.throws(() => readLive(bad, saved, true)) }
  assert.throws(() => readLive(good, saved, false), /Review cannot/)
  const waiting = status(); waiting.decision = { kind: 'waiting' }; assert.doesNotThrow(() => readActivationStatus(waiting))
  waiting.decision.settleRemainingMs = 1; assert.throws(() => readActivationStatus(waiting), /integer/)
})

function harness() {
  let saved = savedSettings(), current = status(saved), writeError: unknown = null, hold: ((v: unknown) => void) | null = null, heldPath = ''
  const calls: { path: string; body: unknown }[] = []
  const s = new ActivationSession({ async post(path, body) {
    calls.push({ path, body: copyDsp(body ?? {}) })
    if (path === heldPath) return await new Promise(resolve => { hold = resolve })
    if (path.endsWith('/settings/read')) return copyDsp(saved)
    if (path.endsWith('/status')) return copyDsp(current)
    if (path.endsWith('/settings/write')) {
      if (writeError) throw writeError
      const b = body as { expectedSettingsRevision: number; settings: SettingsSnapshot['settings'] }
      assert.equal(b.expectedSettingsRevision, saved.settingsRevision)
      saved = { ...saved, settingsRevision: saved.settingsRevision + 1, settings: copyDsp(b.settings) }; current = status(saved); return copyDsp(saved)
    }
    if (path.endsWith('/review') || path.endsWith('/live')) {
      const live = liveFixture(saved, path.endsWith('/review')); current = live.status; return copyDsp(live)
    }
    if (path.endsWith('/arm')) { current = { ...current, armed: true, review: null, decision: { kind: 'armed' } }; return copyDsp(current) }
    if (path.endsWith('/disarm')) { current = { ...current, armed: false, review: null, decision: { kind: 'disarmed' } }; return copyDsp(current) }
    if (path.endsWith('/manual')) {
      const command = body as { mode: string; durationMs?: number }
      current = { ...current, review: null, manualRevision: current.manualRevision + 1, manualOverride: command.mode === 'clear' ? null : command.mode === 'for' ? { mode: 'for', untilMs: 1000 + command.durationMs! } : { mode: 'until-cleared' } }
      return copyDsp(current)
    }
    throw Error(path)
  } }, () => 10000)
  return { s, calls, setWriteError(e: unknown) { writeError = e }, advance() { saved.settingsRevision++; current = status(saved) }, setStatus(v: Status) { current = v }, hold(path: string) { heldPath = path }, release(v: unknown) { heldPath = ''; hold!(v) } }
}

test('connection is lazy; reads and conflicts preserve local drafts and writes require fresh current revision without retries', async () => {
  const h = harness(), s = h.s
  assert.equal(h.calls.length, 0)
  await s.read(); assert.deepEqual(h.calls.map(c => c.path), ['/api/automation/settings/read', '/api/automation/status'])
  s.edit(d => { d.observer.timezone = 'Asia/Tokyo' })
  h.advance(); await s.read(); assert.equal(s.draft.observer.timezone, 'Asia/Tokyo'); assert.equal(s.saved!.settingsRevision, 5)
  h.setWriteError(new ApiError(409, 'store-conflict', 'Another writer saved settings'))
  await s.save(); assert.equal(s.needsRead, true); assert.equal(s.draft.observer.timezone, 'Asia/Tokyo')
  const count = h.calls.length; await s.save(); assert.equal(h.calls.length, count)
  await s.read(); h.setWriteError(null); await s.save(); assert.equal(s.saved!.settingsRevision, 6); assert.equal(s.dirty, false)
  assert.equal((h.calls.filter(c => c.path.endsWith('/write'))[1]!.body as { expectedSettingsRevision: number }).expectedSettingsRevision, 5)
})

test('offline, uncertain and malformed write acknowledgments never erase a settings draft or auto-retry', async () => {
  for (const error of [new Error('Offline'), new ApiError(500, 'commit-uncertain', 'May have committed')]) {
    const h = harness(); await h.s.read(); h.s.edit(d => { d.observer.hostLabel = 'Keep me' }); h.setWriteError(error)
    await h.s.save(); assert.equal(h.s.draft.observer.hostLabel, 'Keep me'); assert.equal(h.s.needsRead, true)
    const count = h.calls.length; await h.s.save(); assert.equal(h.calls.length, count)
  }
  const h = harness(); await h.s.read(); h.s.edit(d => { d.observer.hostLabel = 'Keep me' }); h.hold('/api/automation/settings/write')
  const saving = h.s.save(); h.release({ ...savedSettings(), settingsRevision: 100 }); await saving
  assert.equal(h.s.needsRead, true); assert.equal(h.s.draft.observer.hostLabel, 'Keep me'); assert.match(h.s.error, /Unexpected saved revision/)
})

test('imported revisions never become write authority; stale status cannot restore a review; builder changes discard delayed reviews', async () => {
  const h = harness(); await h.s.read()
  const imported = savedSettings(); imported.settingsRevision = 9999; imported.settings.observer.hostLabel = 'Local only'
  h.s.import(dspJson(imported)); assert.equal(h.s.saved!.settingsRevision, 4); assert.equal(h.calls.length, 2)
  await h.s.save(); assert.equal((h.calls.find(c => c.path.endsWith('/write'))!.body as { expectedSettingsRevision: number }).expectedSettingsRevision, 4)
  await h.s.capture(true); assert.equal(h.s.reviewCurrent, true)
  h.setStatus({ ...status(h.s.saved!), manualRevision: 2 }); await h.s.refreshStatus(); assert.equal(h.s.review, null)
  h.setStatus(status(h.s.saved!)); await h.s.refreshStatus(); assert.match(h.s.statusError, /Stale/)
  const j = harness(); await j.s.read(); j.hold('/api/automation/review'); const pending = j.s.capture(true)
  j.s.builderChanged(); j.release(liveFixture()); await pending; assert.equal(j.s.review, null); assert.equal(j.s.live, null)
})

test('disarm stays independent of a blocked live request and late live cannot restore authority', async () => {
  const h = harness(); await h.s.read(); h.hold('/api/automation/review')
  const pending = h.s.capture(true); assert.equal(h.s.busy, true)
  await h.s.disarm(); assert.equal(h.s.status!.armed, false)
  h.release(liveFixture()); await pending; assert.equal(h.s.review, null); assert.equal(h.s.live, null)
  assert.equal(h.calls.filter(c => c.path.endsWith('/disarm')).length, 1)
})

test('manual ownership pause retains armed state, clear never grants a disarmed policy, exact durations validate before POST', async () => {
  const h = harness(); await h.s.read(); await h.s.capture(true); await h.s.arm(); assert.equal(h.s.status!.armed, true)
  await h.s.manual({ mode: 'until-cleared' }); assert.equal(h.s.status!.armed, true); assert.equal(h.s.status!.manualOverride!.mode, 'until-cleared')
  await h.s.manual({ mode: 'for', durationMs: 5000 }); assert.deepEqual(h.s.status!.manualOverride, { mode: 'for', untilMs: 6000 })
  const count = h.calls.length; await h.s.manual({ mode: 'for', durationMs: 1.1 }); assert.equal(h.calls.length, count)
  await h.s.disarm(); await h.s.manual({ mode: 'clear' }); assert.equal(h.s.status!.armed, false); assert.equal(h.s.status!.manualOverride, null)
})

test('review expiry gates only starting arm, edits and revision changes revoke client review and missing inventory never passes', async () => {
  let now = 0
  const s = new ActivationSession({ async post(path) { if (path.endsWith('/settings/read')) return savedSettings(); if (path.endsWith('/status')) return status(); return liveFixture() } }, () => now)
  await s.read(); await s.capture(true); assert.equal(s.reviewCurrent, true)
  now = 300001; assert.equal(s.reviewCurrent, false); await s.arm(); assert.match(s.error, /expired/)
  now = 0; await s.capture(true); s.edit(d => { d.transitionPolicy.settleMs = 3 }); assert.equal(s.review, null)
  const v = liveFixture(savedSettings(), false); v.inventory = null; v.status.inventoryRevision = null; v.blocked = { code: 'inventory-unavailable', message: 'No runtime inventory' }; v.status.blocked = v.blocked
  assert.equal(readLive(v, savedSettings(), false).inventory, null)
})

test('automation API rejects fractional revision lexemes even when JSON.parse would round them, retaining Float64 payloads', async () => {
  const api = new Api(async () => new Response(dspJson(status()).replace('"settingsRevision":4', '"settingsRevision":4.0000000000000001')))
  api.token = 'fixture'
  await assert.rejects(api.post('/api/automation/status', {}), /integer literal/)
})


test('edits during settings reads/writes survive acknowledgments and builder edits cannot clobber settings reads', async () => {
  const h = harness(); await h.s.read(); h.hold('/api/automation/settings/write')
  h.s.edit(d => { d.observer.hostLabel = 'Submitted' }); const pending = h.s.save()
  const ack = savedSettings(); ack.settingsRevision++; ack.settings.observer.hostLabel = 'Submitted'
  h.s.edit(d => { d.observer.hostLabel = 'Later local edit' })
  h.setStatus(status(ack)); h.release(ack); await pending
  assert.equal(h.s.saved!.settings.observer.hostLabel, 'Submitted'); assert.equal(h.s.draft.observer.hostLabel, 'Later local edit'); assert.equal(h.s.dirty, true)
  const j = harness(); j.hold('/api/automation/settings/read'); const reading = j.s.read()
  j.s.builderChanged(); j.release(savedSettings()); await reading
  assert.equal(j.s.draft.graph!.name, 'Saved headphones'); assert.equal(j.s.dirty, false)
})


test('explicit settings reconciliation establishes a fresh process status baseline without losing drafts', async () => {
  const h = harness(); await h.s.read(); await h.s.manual({ mode: 'until-cleared' })
  h.s.edit(d => { d.transitionPolicy.settleMs = 777 })
  h.setStatus(status()); await h.s.refreshStatus(); assert.match(h.s.statusError, /Stale/)
  await h.s.read()
  assert.equal(h.s.status!.manualRevision, 0); assert.equal(h.s.statusError, '')
  assert.equal(h.s.draft.transitionPolicy.settleMs, 777); assert.equal(h.s.dirty, true)
  assert.equal(h.s.review, null)
})

test('reviews bind prospective high-water revision and canonical plan identities to every candidate', () => {
  const saved = savedSettings(), good = liveFixture(saved)
  const cases: ((v: ReturnType<typeof liveFixture>) => void)[] = [
    v => { for (const a of v.review!.actions) { a.candidate.revision = 0; a.preview.plan.configRevision = 0 } },
    v => { v.review!.actions[1]!.candidate.revision = 3; v.review!.actions[1]!.preview.plan.configRevision = 3 },
    v => { (v.review!.actions[0]!.preview.plan.sources as any[])[0].selector.deviceUid = 'wrong' },
    v => { (v.review!.actions[0]!.preview.plan.outputs as any[])[0].target.deviceUid = 'wrong' },
    v => { (v.review!.actions[0]!.preview.plan.outputs as any[])[0].mix = 'wrong' },
    v => { (v.review!.actions[0]!.preview.plan.outputs as any[])[0].gain = 0.25 },
    v => { (v.review!.actions[0]!.preview.plan.sends as any[])[0].mute = true },
    v => { (v.review!.actions[0]!.preview.plan.mixes as any[])[0].channels = 2 },
  ]
  for (const mutate of cases) {
    const bad = copyDsp(good); mutate(bad)
    // Keep the mirrored unavailable rows internally consistent to exercise
    // candidate binding, not merely the existing mirror comparison.
    for (const a of bad.review!.actions) {
      a.preview.unavailableIdentities.sources = copyDsp((a.preview.plan.sources as any[]).filter(s => s.status !== 'selected'))
      a.preview.unavailableIdentities.outputs = copyDsp((a.preview.plan.outputs as any[]).filter(s => s.status !== 'selected'))
    }
    assert.throws(() => readLive(bad, saved, true))
  }
  const live = copyDsp(good); live.review = null; live.status.review = null
  live.preview = copyDsp(good.review!.actions[0]!)
  live.preview.candidate.revision = 1; live.preview.preview.plan.configRevision = 1
  assert.throws(() => readLive(live, saved, false))
})
