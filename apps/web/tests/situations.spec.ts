import { readFileSync } from 'node:fs'
import { test, expect } from '@playwright/test'
import type { Page } from '@playwright/test'
import { copyDsp, dspJson } from '../src/dsp-json.ts'
import { buildPreset, emptyPreset, emptyRequest } from '../src/situations.ts'
import type { SituationRequest } from '../src/situations.ts'

// The service boundary is mocked, not an alternative JS evaluator. These explicit
// unknown-path fixtures exercise UI preservation, interpretation, and race guards.
function evaluation(r: SituationRequest) {
  const node = r.graph.nodes.find(n => n.id === r.graph.root)!, terminal = node.kind === 'decision' ? node.onUnknown : node.id
  const fact = r.snapshot.facts[0]
  return { mode: 'simulation', evaluation: { schemaVersion: 1, token: { graphId: r.graph.id, graphRevision: r.graph.revision, snapshotRevision: r.snapshot.revision, epoch: r.snapshot.epoch, evaluatedAtMs: r.nowMs, context: r.context }, terminal, action: { kind: 'no-change' }, observations: fact ? [fact] : [], trace: [ ...(node.kind === 'decision' && fact ? [{ kind: 'fact', node: node.id, detail: { key: fact.key, predicate: { op: 'contains', value: 'device/HEADPHONES' }, result: 'unknown', observationIndex: 0, ageMs: 0, fresh: true, unknownReason: 'incomplete-observation' } }] : []), ...(node.kind === 'decision' ? [{ kind: 'decision', node: node.id, result: 'unknown', next: terminal }] : []), { kind: 'terminal', node: terminal } ], traceTruncated: !!fact, audioApplied: false } }
}
async function mockApi(page: Page) {
  const requests: { path: string; body: SituationRequest }[] = []
  let offline = false, invalid = false, held: (() => void) | null = null, hold = false
  await page.route('**/api/**', async route => {
    const path = new URL(route.request().url()).pathname, body = route.request().method() === 'POST' ? JSON.parse(route.request().postData()!) : {}
    const reply = (json: unknown, status = 200) => route.fulfill({ status, contentType: 'application/json', body: dspJson(json) })
    if (path === '/api/session') return reply({ apiVersion: 1, csrfToken: 'test-session', audioAvailable: false })
    expect(route.request().headers()['x-mixyard-token']).toBe('test-session')
    if (path === '/api/status') return reply({ connected: false, snapshot: null, error: null })
    if (path === '/api/profiles/read') return reply({ format: 'mixyard-local-drafts', schemaVersion: 1, storeRevision: 7, configRevision: 1, profiles: [{ id: 'profile/opaque-HEAD', name: 'Headphones', config: config() }, { id: 'profile/opaque-NORMAL', name: 'Normal', config: config() }], draft: config(), previousDraft: null })
    if (path.startsWith('/api/automation/')) requests.push({ path, body })
    if (path === '/api/automation/validate') return invalid ? reply({ error: { code: 'invalid-graph', message: 'Unsupported imported field: rejectedExtra' } }, 422) : reply({ valid: true })
    if (path === '/api/automation/evaluate') { if (hold) await new Promise<void>(r => { held = r }); return reply(evaluation(body)) }
    if (path === '/api/automation/context') {
      if (offline) return reply({ error: { code: 'offline-provider', message: 'Context collector offline' } }, 503)
      return reply({ mode: 'read-only-live-context', snapshot: { schemaVersion: 1, revision: 3, epoch: 'service-epoch', facts: [{ key: { name: 'device.connected' }, status: 'unknown', provider: 'devices', evidence: 'platform-observed', observedAtMs: 0, validForMs: 5000, completeness: 'partial', reason: 'More than 256 device identities; no truncated known set' }] }, nowMs: 0, catalog: emptyRequest().catalog, applicationScope: ['application/MAPPED'], providers: [{ provider: 'devices', status: 'partial', reason: 'More than 256 device identities; unknown set' }, { provider: 'application.fullscreen', status: 'unsupported', reason: 'No window provider' }] })
    }
    throw new Error(`Unexpected API call ${path}`)
  })
  return { requests, offline(value: boolean) { offline = value }, invalid(value: boolean) { invalid = value }, hold() { hold = true }, release() { hold = false; held?.() } }
}
function config() { return JSON.parse(readFileSync(new URL('../src/example-config.json', import.meta.url), 'utf8')) }
async function open(page: Page) { await page.goto('/'); await expect(page.getByRole('button', { name: 'Refresh', exact: true })).toBeEnabled(); await page.getByRole('button', { name: 'Situations', exact: true }).click() }
async function preset(page: Page) {
  await page.getByLabel('Exact device UID', { exact: true }).fill('device/HEADPHONES')
  await page.getByLabel('Headphone profile ID', { exact: true }).fill('profile/opaque-HEAD')
  await page.getByLabel('Normal profile ID', { exact: true }).fill('profile/opaque-NORMAL')
  await page.getByLabel('Speaker output ID to mute', { exact: true }).fill('speakers/EXACT')
  await page.getByRole('button', { name: 'Create preset graph', exact: true }).click()
}
async function exportRequest(page: Page) { const event = page.waitForEvent('download'); await page.getByRole('button', { name: 'Export full configuration', exact: true }).first().click(); return JSON.parse(readFileSync((await (await event).path())!, 'utf8')) as SituationRequest }
async function expert(page: Page) { await page.getByText('Expert JSON / import full configuration', { exact: true }).click() }

test('headphone preset uses saved opaque profile IDs, graph forms relink exact IDs and block referenced deletion', async ({ page }) => {
  const api = await mockApi(page); await open(page)
  await expect(page.getByRole('button', { name: 'Create preset graph', exact: true })).toBeDisabled()
  await expect(page.getByText('Preset incomplete.', { exact: false })).toContainText('Headphone profile ID')
  await expect(page.locator('#preset-profiles option[value="profile/opaque-HEAD"]')).toHaveText('Headphones')
  await preset(page)
  await expect(page.getByRole('img', { name: 'My situations decision graph' })).toBeVisible()
  for (const truth of ['true', 'false', 'unknown']) await expect(page.getByRole('button', { name: `when: ${truth} to ${truth === 'true' ? 'active' : truth === 'false' ? 'normal' : 'safe'}`, exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Edit safe', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Delete node safe', exact: true })).toBeDisabled()
  await page.getByLabel('Node ID', { exact: true }).fill(' SAFE / Ω ')
  await page.getByRole('button', { name: 'Rename and relink references', exact: true }).click()
  await expect(page.getByRole('button', { name: 'when: unknown to  SAFE / Ω', exact: false })).toBeVisible()
  await page.getByRole('button', { name: 'Add terminal', exact: true }).click()
  await page.getByRole('button', { name: 'Edit when', exact: true }).click()
  await page.getByLabel('When false', { exact: true }).selectOption('node-1')
  await page.getByRole('button', { name: 'Edit normal', exact: true }).click()
  await page.getByRole('button', { name: 'Delete node normal', exact: true }).click()
  await page.getByRole('button', { name: 'Simulate configuration', exact: true }).click()
  await expect(page.getByText('Simulation only. No change.', { exact: false })).toBeVisible()
  const r = [...api.requests].reverse().find(r => r.path.endsWith('/evaluate'))!.body
  expect(r.graph.nodes[0]).toMatchObject({ onFalse: 'node-1', onUnknown: ' SAFE / Ω ' })
  expect(r.graph.nodes[1]).toMatchObject({ action: { profileId: 'profile/opaque-HEAD', overrides: [{ scope: 'output', output: 'speakers/EXACT', mute: true }] } })
  expect(r.graph.nodes.some(n => n.id === 'normal')).toBe(false)
  expect((await exportRequest(page)).graph).toEqual(r.graph)
  await page.screenshot({ path: 'test-results/situations-desktop.png', fullPage: true })
})

test('compound typed conditions and scoped actions are editable through ordinary forms', async ({ page }) => {
  const api = await mockApi(page); await open(page); await preset(page)
  const root = page.getByRole('group', { name: 'Condition', exact: true })
  await root.getByLabel('Condition type', { exact: true }).selectOption('all')
  await page.getByRole('button', { name: 'Add condition to Condition', exact: true }).click()
  const second = page.getByRole('group', { name: 'Condition.2', exact: true })
  await second.getByLabel('Fact', { exact: true }).selectOption('power.battery-percent')
  await second.getByLabel('Comparison', { exact: true }).selectOption('range')
  await second.getByLabel('Inclusive minimum', { exact: true }).fill('20.000000000000004')
  await second.getByLabel('Inclusive maximum', { exact: true }).fill('80')
  await page.getByRole('button', { name: 'Add condition to Condition', exact: true }).click()
  const third = page.getByRole('group', { name: 'Condition.3', exact: true })
  await third.getByLabel('Fact', { exact: true }).selectOption('application.running')
  await third.getByLabel('Comparison', { exact: true }).selectOption('contains-all')
  await third.getByRole('button', { name: 'Add Exact member ID', exact: true }).click()
  await third.getByLabel('Exact member ID 1', { exact: true }).fill('application/OPAQUE')
  await third.getByLabel('Condition type', { exact: true }).selectOption('not')
  await page.getByRole('button', { name: 'Edit active', exact: true }).click()
  await page.getByLabel('Attenuation gain (optional, 0–1)', { exact: true }).fill('0.9999999999999999')
  await page.getByRole('button', { name: 'Simulate configuration', exact: true }).click()
  await expect(page.getByText('Simulation only. No change.', { exact: false })).toBeVisible()
  const r = [...api.requests].reverse().find(r => r.path.endsWith('/evaluate'))!.body, condition = r.graph.nodes[0]!.kind === 'decision' && r.graph.nodes[0]!.condition
  expect(condition).toMatchObject({ kind: 'all', conditions: [{ kind: 'fact' }, { kind: 'fact', predicate: { op: 'range', min: 20.000000000000004, max: 80 } }, { kind: 'not', condition: { predicate: { op: 'contains-all', values: ['application/OPAQUE'] } } }] })
  expect(r.graph.nodes[1]).toMatchObject({ action: { overrides: [{ gain: 0.9999999999999999 }] } })
})

test('hardware monitor preset uses an explicit endpoint group and suppresses only its chosen monitor send', async ({ page }) => {
  await mockApi(page); await open(page)
  await page.getByLabel('Preset', { exact: true }).selectOption('monitor')
  await page.getByLabel('Hardware-monitor declaration endpoint UID', { exact: true }).fill('interface/INPUT')
  await page.getByLabel('Interface endpoint group IDs (one per line; blank uses declaration UID)', { exact: true }).fill('interface/INPUT\ninterface/OUTPUT')
  await page.getByLabel('Mic source ID', { exact: true }).fill('source/mic')
  await page.getByLabel('Monitor mix ID', { exact: true }).fill('mix/monitor')
  await expect(page.getByRole('button', { name: 'Create preset graph', exact: true })).toBeDisabled()
  await page.getByLabel('I explicitly declare the chosen endpoint has hardware monitoring (simulation claim)', { exact: true }).check()
  await page.getByRole('button', { name: 'Create preset graph', exact: true }).click()
  const r = await exportRequest(page)
  expect(r.graph.nodes[0]).toMatchObject({ condition: { kind: 'all', conditions: [{ key: { name: 'device.connected' }, predicate: { op: 'contains-all', values: ['interface/INPUT', 'interface/OUTPUT'] } }, { predicate: { op: 'count', min: 2, max: 2 } }, { key: { name: 'device.hardware-monitor-declared', subject: 'interface/INPUT' } }] } })
  expect(r.graph.nodes[1]).toMatchObject({ action: { kind: 'select', overrides: [{ scope: 'send', source: 'source/mic', mix: 'mix/monitor', mute: true }] } })
  expect(r.snapshot.facts[0]).toMatchObject({ evidence: 'user-declared', provider: 'user-simulation' })
  await page.screenshot({ path: 'test-results/situations-endpoint-group.png', fullPage: true })
})

test('full import validates before replacement, preserves raw parameters/signed zero and displays shared evidence/truncation', async ({ page }) => {
  const api = await mockApi(page); await open(page); await expert(page)
  const r = buildPreset({ ...emptyPreset(), device: 'device/HEADPHONES', profile: 'profile/opaque-HEAD', normalProfile: 'profile/opaque-NORMAL', output: 'speakers/EXACT' }, emptyRequest())
  r.raw = { signed: -0, low: 1.0000000000000002, high: 9007199254740994 }; r.graph.rawGraph = ['retain']; r.graph.name = 'Imported complete graph'
  r.snapshot.facts = [{ key: { name: 'device.connected' }, status: 'known', value: { type: 'text-set', value: ['device/HEADPHONES'] }, provider: 'user claim', evidence: 'platform-observed', observedAtMs: 0, validForMs: 5000, completeness: 'partial', reason: 'incomplete fixture', rawFact: -0 }]
  const json = page.getByLabel('Full situation configuration JSON', { exact: true })
  await json.fill(dspJson(r)); api.invalid(true)
  await page.getByRole('button', { name: 'Validate and import JSON', exact: true }).click()
  await expect(page.getByRole('alert').filter({ hasText: 'Unsupported imported field' })).toBeVisible()
  await expect(page.getByLabel('Graph name', { exact: true })).toHaveValue('My situations')
  api.invalid(false); await page.getByRole('button', { name: 'Validate and import JSON', exact: true }).click()
  await expect(page.getByLabel('Graph name', { exact: true })).toHaveValue('Imported complete graph')
  await page.getByRole('button', { name: 'Edit active', exact: true }).click()
  await page.getByLabel('Profile ID (optional with reductions)', { exact: true }).fill('profile/new-opaque')
  const exported = await exportRequest(page); expect(exported.raw).toEqual(r.raw); expect(exported.snapshot).toEqual(r.snapshot); expect(exported.graph.rawGraph).toEqual(['retain'])
  await page.getByRole('button', { name: 'Routing', exact: true }).click()
  await page.getByRole('button', { name: 'Situations', exact: true }).click()
  expect(await exportRequest(page)).toEqual(exported)
  await page.getByRole('button', { name: 'Simulate configuration', exact: true }).click()
  await expect(page.getByText('incomplete-observation / fresh / age 0 ms.', { exact: true })).toBeVisible()
  await expect(page.getByRole('heading', { name: 'Evidence #1: device.connected', exact: true })).toBeVisible()
  await expect(page.getByText('Trace truncated by the core', { exact: false })).toBeVisible()
  await expect(page.getByText('claimed platform-observed. Supplied simulation evidence;', { exact: false })).toBeVisible()
  await expect(page.locator('.situation-edge.evaluated')).toHaveCount(1)
  await json.fill('{'); await expect(page.getByText('No current trace.', { exact: false })).toBeVisible()
  await expect(page.locator('.situation-edge.evaluated')).toHaveCount(0)
})

test('invalid references/cycles and unsafe metadata are rejected while keeping the previous full configuration', async ({ page }) => {
  const api = await mockApi(page); await open(page); await expert(page)
  const r = buildPreset({ ...emptyPreset(), device: 'd', profile: 'p', normalProfile: 'n', output: 'o' }, emptyRequest())
  const n = r.graph.nodes[0]!; if (n.kind !== 'decision') throw Error('fixture')
  n.onUnknown = 'when'
  const json = page.getByLabel('Full situation configuration JSON', { exact: true }); await json.fill(dspJson(r)); await page.getByRole('button', { name: 'Validate and import JSON', exact: true }).click()
  await expect(page.getByRole('alert').filter({ hasText: 'cycle' })).toBeVisible(); expect(api.requests).toHaveLength(0)
  n.onUnknown = 'missing'; await json.fill(dspJson(r)); await page.getByRole('button', { name: 'Validate and import JSON', exact: true }).click(); await expect(page.getByRole('alert').filter({ hasText: 'missing branch target' })).toBeVisible()
  n.onUnknown = 'safe'; r.snapshot.revision = 9007199254740994; await json.fill(dspJson(r)); await page.getByRole('button', { name: 'Validate and import JSON', exact: true }).click(); await expect(page.getByRole('alert').filter({ hasText: 'exact nonnegative integer' })).toBeVisible()
  await expect(page.getByLabel('Root node', { exact: true })).toHaveValue('safe')
})

test('context provider gaps/offline recovery do not replace edits until explicit copy into simulation', async ({ page }) => {
  const api = await mockApi(page); await open(page)
  await page.getByLabel('Graph name', { exact: true }).fill('Keep my work')
  api.offline(true); await page.getByRole('button', { name: 'Read live context', exact: true }).click()
  await expect(page.getByRole('alert').filter({ hasText: 'Context collector offline' })).toBeVisible()
  await expect(page.getByLabel('Graph name', { exact: true })).toHaveValue('Keep my work')
  api.offline(false); await page.getByRole('button', { name: 'Read live context', exact: true }).click()
  await expect(page.getByText('An unmapped application remains unknown;', { exact: false })).toBeVisible()
  await page.getByText('Application mapping domain (1)', { exact: true }).click()
  await expect(page.getByText('application/MAPPED', { exact: true })).toBeVisible()
  await expect(page.getByText('devices: partial', { exact: true })).toBeVisible(); await expect(page.getByText('application.fullscreen: unsupported', { exact: true })).toBeVisible()
  await expect(page.getByLabel('Snapshot epoch', { exact: true })).toHaveValue('simulation')
  await page.getByRole('button', { name: 'Copy read-only context into simulation', exact: true }).click()
  await expect(page.getByLabel('Snapshot epoch', { exact: true })).toHaveValue('service-epoch')
  await expect(page.getByLabel('Graph name', { exact: true })).toHaveValue('Keep my work')
  const r = await exportRequest(page); expect(r.context.storeRevision).toBe(7); expect(r.snapshot.facts[0]).toMatchObject({ status: 'unknown', completeness: 'partial' })
})

test('in-flight simulation cannot overwrite later node edits', async ({ page }) => {
  const api = await mockApi(page); await open(page); await preset(page); api.hold()
  await page.getByRole('button', { name: 'Simulate configuration', exact: true }).click()
  await expect.poll(() => api.requests.some(r => r.path.endsWith('/evaluate'))).toBe(true)
  await page.getByLabel('Graph name', { exact: true }).fill('After request')
  api.release(); await expect(page.getByRole('button', { name: 'Simulate configuration', exact: true })).toBeEnabled()
  await expect(page.getByText('No current trace.', { exact: false })).toBeVisible(); await expect(page.locator('.situation-edge.evaluated')).toHaveCount(0)
})

test('390px editor supports keyboard branch navigation, focus and forms without page overflow', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 }); await mockApi(page); await open(page); await preset(page)
  const target = page.getByRole('button', { name: 'when: unknown to safe', exact: true }); await target.focus(); await page.keyboard.press('Enter')
  await expect(page.getByRole('group', { name: 'Edit node safe', exact: true })).toBeVisible()
  const id = page.getByLabel('Node ID', { exact: true }); await id.focus(); await page.keyboard.press('ControlOrMeta+A'); await page.keyboard.type('safe/keyboard'); await page.keyboard.press('Tab'); await page.keyboard.press('Enter')
  await expect(page.getByRole('button', { name: 'when: unknown to safe/keyboard', exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Edit when', exact: true }).focus(); await page.keyboard.press('Space')
  const branch = page.getByLabel('When true', { exact: true }); await branch.focus(); await page.keyboard.press('End'); await page.keyboard.press('Enter')
  await expect(branch).toHaveValue('safe/keyboard')
  await expect(page.getByRole('region', { name: 'Decision graph diagram;', exact: false })).toHaveAttribute('tabindex', '0')
  const size = await page.evaluate(() => ({ width: document.documentElement.scrollWidth, viewport: innerWidth, diagram: document.querySelector('.situation-diagram')!.scrollWidth, region: document.querySelector('.situation-diagram')!.clientWidth }))
  expect(size.width).toBeLessThanOrEqual(size.viewport); expect(size.diagram).toBeGreaterThan(size.region)
  await page.screenshot({ path: 'test-results/situations-mobile.png', fullPage: true })
})

test('simulation fact forms keep declared evidence/status/time metadata and negative unknown paths distinct', async ({ page }) => {
  const api = await mockApi(page); await open(page); await preset(page)
  await page.getByText('Simulation facts (0)', { exact: true }).click()
  await page.getByRole('button', { name: 'Add simulation fact', exact: true }).click()
  const fact = page.getByRole('group', { name: 'Fact 1', exact: true })
  await fact.getByLabel('Fact name', { exact: true }).selectOption('device.hardware-monitor-declared')
  await fact.getByLabel('Fact subject ID', { exact: true }).fill('interface/EXACT')
  await fact.getByLabel('Fact status', { exact: true }).selectOption('known')
  await fact.getByLabel('Fact value', { exact: true }).selectOption('true')
  await fact.getByLabel('Evidence claim', { exact: true }).selectOption('user-declared')
  await fact.getByLabel('Observed at (epoch ms; blank = unknown)', { exact: true }).fill('0')
  await fact.getByLabel('Valid for (ms)', { exact: true }).fill('1234')
  await fact.getByLabel('Completeness', { exact: true }).selectOption('partial')
  await fact.getByLabel('Reason', { exact: true }).fill('Explicit user simulation claim')
  await page.getByRole('button', { name: 'Simulate configuration', exact: true }).click()
  await expect(page.getByText('Simulation only. No change.', { exact: false })).toBeVisible()
  let r = [...api.requests].reverse().find(r => r.path.endsWith('/evaluate'))!.body
  expect(r.snapshot.facts[0]).toMatchObject({ key: { name: 'device.hardware-monitor-declared', subject: 'interface/EXACT' }, status: 'known', evidence: 'user-declared', observedAtMs: 0, validForMs: 1234, completeness: 'partial', reason: 'Explicit user simulation claim', value: { type: 'bool', value: true } })
  await fact.getByLabel('Fact status', { exact: true }).selectOption('ambiguous')
  await expect(page.getByText('No current trace.', { exact: false })).toBeVisible()
  r = await exportRequest(page); expect(r.snapshot.facts[0]!.status).toBe('ambiguous'); expect(r.snapshot.facts[0]!.value).toBeUndefined()
})

test('admitted depth-32 conditions remain keyboard editable without mobile page overflow', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 }); await mockApi(page); await open(page); await expert(page)
  const r = buildPreset({ ...emptyPreset(), device: 'd', profile: 'p', normalProfile: 'n', output: 'o' }, emptyRequest()), n = r.graph.nodes[0]!
  if (n.kind !== 'decision') throw Error('fixture')
  for (let i = 0; i < 31; i++) n.condition = { kind: 'not', condition: n.condition }
  await page.getByLabel('Full situation configuration JSON', { exact: true }).fill(dspJson(r))
  await page.getByRole('button', { name: 'Validate and import JSON', exact: true }).click()
  await expect(page.getByLabel('Root node', { exact: true })).toHaveValue('when')
  const member = page.getByLabel('Exact member ID', { exact: true }); await member.focus(); await page.keyboard.press('End'); await page.keyboard.type('/deep')
  await expect(member).toHaveValue('d/deep')
  const size = await page.evaluate(() => ({ width: document.documentElement.scrollWidth, viewport: innerWidth }))
  expect(size.width).toBeLessThanOrEqual(size.viewport)
  expect((await exportRequest(page)).graph.nodes[0]).toMatchObject({ kind: 'decision' })
})


test('valid imports and context replacements clear invalid DOM drafts, integer metadata never rounds', async ({ page }) => {
  const api = await mockApi(page); await open(page)
  const clock = page.getByLabel('Evaluate at (epoch ms)', { exact: true })
  await clock.fill('-1')
  await expert(page)
  const imported = emptyRequest(); imported.nowMs = 12
  await page.getByLabel('Full situation configuration JSON', { exact: true }).fill(dspJson(imported))
  await page.getByRole('button', { name: 'Validate and import JSON', exact: true }).click()
  await expect(clock).toHaveValue('12')
  await expect.poll(() => clock.evaluate(e => (e as HTMLInputElement).validity.valid)).toBe(true)
  await page.getByRole('button', { name: 'Simulate configuration', exact: true }).click()
  await expect(page.getByText('Simulation only. No change.', { exact: false })).toBeVisible()
  await clock.fill('1.0000000000000001')
  const before = api.requests.filter(r => r.path.endsWith('/evaluate')).length
  await page.getByRole('button', { name: 'Simulate configuration', exact: true }).click()
  expect(api.requests.filter(r => r.path.endsWith('/evaluate')).length).toBe(before)
  await page.getByRole('button', { name: 'Read live context', exact: true }).click()
  await page.getByRole('button', { name: 'Copy read-only context into simulation', exact: true }).click()
  await expect.poll(() => clock.evaluate(e => (e as HTMLInputElement).validity.valid)).toBe(true)
})


test('switching numeric decision owners discards stale validity and complete response contradictions are rejected', async ({ page }) => {
  const api = await mockApi(page); await open(page); await expert(page)
  const request = emptyRequest()
  request.graph.root = 'first'
  request.graph.nodes.unshift(...(['first', 'second'] as const).map((id, i) => ({ id, kind: 'decision' as const, condition: { kind: 'fact' as const, key: { name: 'civil.minute-of-day' }, predicate: { op: 'lt', value: (i + 1) * 10 } }, onTrue: 'safe', onFalse: 'safe', onUnknown: 'safe' })))
  await page.getByLabel('Full situation configuration JSON', { exact: true }).fill(dspJson(request))
  await page.getByRole('button', { name: 'Validate and import JSON', exact: true }).click()
  await expect(page.getByText('Full configuration imported after core validation', { exact: false })).toBeVisible()
  const numeric = page.getByLabel('Numeric comparison value', { exact: true })
  await numeric.fill('')
  await expect.poll(() => numeric.evaluate(e => (e as HTMLInputElement).validity.valid)).toBe(false)
  await page.getByRole('button', { name: 'Edit second', exact: true }).click()
  await expect(numeric).toHaveValue('20')
  await expect.poll(() => numeric.evaluate(e => (e as HTMLInputElement).validity.valid)).toBe(true)
  await page.getByRole('button', { name: 'Simulate configuration', exact: true }).click()
  await expect(page.locator('.situation-result .message.success')).toContainText('Terminal: safe.')
  expect(api.requests.filter(r => r.path.endsWith('/evaluate')).length).toBeGreaterThan(1)
  await page.route('**/api/automation/evaluate', route => {
    const body = JSON.parse(route.request().postData()!) as SituationRequest, result = evaluation(body)
    result.evaluation.trace = []
    result.evaluation.traceTruncated = false
    return route.fulfill({ json: result })
  })
  await page.getByRole('button', { name: 'Simulate configuration', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('Complete trace does not reach its declared result')
  await expect(page.locator('.situation-result .message.success')).toHaveCount(0)
})
