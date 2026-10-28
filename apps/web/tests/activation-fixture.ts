import { readFileSync } from 'node:fs'
import { copyDsp } from '../src/dsp-json.ts'
import { defaultSettings } from '../src/activation.ts'
import type { Live, Review, SettingsSnapshot, Status } from '../src/activation.ts'
import { emptyRequest } from '../src/situations.ts'
import type { SituationRequest } from '../src/situations.ts'
import type { Config, Inventory } from '../src/model.ts'
export function config(): Config { return JSON.parse(readFileSync(new URL('../src/example-config.json', import.meta.url), 'utf8')) }
export function savedSettings(): SettingsSnapshot {
  const r = emptyRequest()
  r.graph.name = 'Saved headphones'
  r.graph.root = 'when'
  r.graph.nodes = [
    { id: 'when', kind: 'decision', condition: { kind: 'fact', key: { name: 'device.connected' }, predicate: { op: 'contains', value: 'device/HEAD' } }, onTrue: 'selected', onFalse: 'second', onUnknown: 'safe' },
    { id: 'selected', kind: 'terminal', action: { kind: 'select', profileId: 'profile/exact', overrides: [] } },
    { id: 'second', kind: 'terminal', action: { kind: 'select', overrides: [{ scope: 'output', output: 'headphones', mute: true }] } },
    { id: 'safe', kind: 'terminal', action: { kind: 'no-change' } },
  ]
  return { format: 'mixyard-situation-settings', schemaVersion: 1, settingsRevision: 4, settings: { ...defaultSettings(), graph: r.graph, catalog: r.catalog } }
}
export function status(saved = savedSettings()): Status {
  return { mode: 'stored-intent-only', armed: false, manualOverride: null, settingsRevision: saved.settingsRevision, storeRevision: 7, configRevision: 1, inventoryRevision: 3, manualRevision: 0, decision: { kind: 'disarmed' }, blocked: null, lastAppliedAtMs: null, review: null, audioApplied: false, backendAvailable: false }
}
export function liveFixture(saved = savedSettings(), wantsReview = true): Live {
  const s = status(saved)
  const candidate: Config = {
    schemaVersion: 2, revision: 2,
    sources: [{ id: 'mic/exact', selector: { kind: 'mic', deviceUid: 'device/MIC' } }],
    mixes: [{ id: 'monitor', name: 'Monitor', channels: 1, master: { gain: 1, mute: false } }],
    sends: [{ source: 'mic/exact', mix: 'monitor', gain: 1, mute: false }],
    outputs: [{ id: 'headphones', mix: 'monitor', target: { kind: 'device', deviceUid: 'device/HEAD' }, gain: 1, mute: true, allowMicMonitor: true }], mediaControls: [],
  }
  const sources = [{ source: 'mic/exact', selector: candidate.sources[0]!.selector, status: 'missing', observedIds: [], format: null }]
  const outputs = [{ ...candidate.outputs[0]!, status: 'missing', observedIds: [], effectiveGain: 0, peakBound: 0, channelMapping: 'identity', rateConversionRequired: null }]
  const inventory: Inventory = { schemaVersion: 2, revision: 3, platform: 'linux', evidence: 'native-discovery', sources: [], outputs: [] }
  const request: SituationRequest = { ...emptyRequest(), graph: saved.settings.graph!, catalog: saved.settings.catalog, snapshot: { schemaVersion: 1, revision: 1, epoch: 'server/epoch', facts: [] }, nowMs: 1000, context: { storeRevision: 7, configRevision: 1, inventoryRevision: 3, manualRevision: 0 }, manualOverride: false }
  const preview = {
    changes: { sources: [], sends: [], mixes: [], masters: [], outputs: [{ identity: ['headphones'], before: { gain: 1 }, after: { gain: 0 } }], mediaControls: [] },
    affectedSensitivePathsBefore: [], affectedSensitivePathsAfter: [],
    sensitivePaths: [{ source: 'mic/exact', selector: { kind: 'mic', deviceUid: 'device/MIC' }, mix: 'monitor', output: 'headphones', target: { kind: 'device', deviceUid: 'device/HEAD' }, reason: 'microphone-monitoring' }],
    unavailableIdentities: { sources, outputs },
    plan: { schemaVersion: 2, mode: 'dry-run', configRevision: 2, inventoryRevision: 3, inventoryEvidence: 'native-discovery', platform: 'linux', sources, sends: [{ ...candidate.sends[0]!, observedId: null, effectiveGain: 0, channelMapping: 'identity', rateConversionRequired: null }], mixes: [{ ...candidate.mixes[0]!, sampleRateHz: 48000, excludeGroups: [], peakBound: 0 }], outputs, diagnostics: [] },
  }
  const actions = saved.settings.graph!.nodes.flatMap(n => n.kind === 'terminal' && n.action.kind === 'select' ? [{ terminal: n.id, action: n.action, candidate, preview }] : [])
  const review: Review | null = wantsReview ? { id: 'opaque-review', expiresAtMs: 301000, authority: { epoch: request.snapshot.epoch, settingsRevision: s.settingsRevision, storeRevision: s.storeRevision, configRevision: s.configRevision, inventoryRevision: 3, manualRevision: 0 }, actions } : null
  if (review) s.review = { id: review.id, expiresAtMs: review.expiresAtMs }
  return copyDsp({ mode: 'stored-intent-only', audioApplied: false, backendAvailable: false, status: s, evaluation: { schemaVersion: 1, audioApplied: false, token: { graphId: request.graph.id, graphRevision: request.graph.revision, snapshotRevision: request.snapshot.revision, epoch: request.snapshot.epoch, evaluatedAtMs: request.nowMs, context: request.context }, terminal: 'safe', action: { kind: 'no-change' }, observations: [], trace: [{ kind: 'decision', node: 'when', result: 'unknown', next: 'safe' }, { kind: 'terminal', node: 'safe' }], traceTruncated: false }, request, context: { mode: 'read-only-live-context', snapshot: request.snapshot, nowMs: request.nowMs, catalog: request.catalog, providers: [{ provider: 'devices', status: 'unsupported', reason: 'Physical inventory unavailable in this fixture' }], applicationScope: [] }, inventory, preview: null, blocked: null, review })
}
