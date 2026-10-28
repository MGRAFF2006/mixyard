import { readFileSync } from 'node:fs'
import { test, expect } from '@playwright/test'
import type { Page } from '@playwright/test'
import { copy, readConfig } from '../src/model.ts'
import type { Snapshot } from '../src/model.ts'

const example = readConfig(JSON.parse(readFileSync(new URL('../src/example-config.json', import.meta.url), 'utf8')))
async function mockApi(page: Page, empty = false) {
  let store: Snapshot = { format: 'mixyard-local-drafts', schemaVersion: 1, storeRevision: 0, configRevision: 1, profiles: [], draft: empty ? null : copy(example), previousDraft: null }
  let conflict = false, fail = false
  const writes: Record<string, unknown>[] = []
  await page.route('**/api/**', async route => {
    const path = new URL(route.request().url()).pathname
    const body = route.request().method() === 'POST' ? route.request().postDataJSON() as Record<string, unknown> : {}
    const reply = (json: unknown, status = 200) => route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(json) })
    if (path === '/api/session') return reply({ apiVersion: 1, csrfToken: 'test-session', audioAvailable: false })
    if (route.request().headers()['x-mixyard-token'] !== 'test-session') return reply({ error: { code: 'token', message: 'Missing token' } }, 403)
    if (path === '/api/status') return reply({ connected: false, snapshot: null, error: null })
    if (path === '/api/profiles/read') return reply(store)
    if (path === '/api/validate') {
      if (fail) return reply({ error: { code: 'invalid-policy', message: 'Headroom exceeds one' } }, 422)
      return reply({ valid: true })
    }
    if (path === '/api/profiles/edit') {
      writes.push(body)
      if (conflict || body.expectedRevision !== store.storeRevision) return reply({ error: { code: 'store-conflict', message: 'Changed by another window' } }, 409)
      const edit = body.edit as { kind: string; config: unknown; id: string | null; name: string }
      store = copy(store); store.storeRevision++
      if (edit.kind === 'draft' || edit.kind === 'load' || edit.kind === 'undo') {
        const next = edit.kind === 'draft' ? readConfig(edit.config) : edit.kind === 'load' ? store.profiles.find(p => p.id === edit.id)!.config : store.previousDraft!
        store.previousDraft = store.draft; store.draft = copy(next); store.draft.revision = ++store.configRevision
      } else if (edit.kind === 'rename') store.profiles.find(p => p.id === edit.id)!.name = edit.name
      else if (edit.kind === 'save') {
        if (edit.id) { const profile = store.profiles.find(p => p.id === edit.id)!; profile.name = edit.name; profile.config = copy(readConfig(edit.config)) }
        else store.profiles.push({ id: `profile-${store.storeRevision}`, name: edit.name, config: copy(readConfig(edit.config)) })
      }
      return reply(store)
    }
    return reply({ error: { code: 'unexpected', message: path } }, 404)
  })
  return { writes, get store() { return store }, conflict() { conflict = true; store.storeRevision++ }, resolve() { conflict = false }, rejectValidation() { fail = true } }
}
async function start(page: Page) { await page.goto('/'); await expect(page.getByRole('button', { name: 'Refresh', exact: true })).toBeEnabled() }
async function gain(page: Page, label: string, value: string) {
  await page.getByRole('slider', { name: label, exact: true }).fill(value)
}

test('fresh store is empty until explicit example; independent sends and output controls save and reopen', async ({ page }) => {
  const api = await mockApi(page, true); await start(page)
  await expect(page.getByText('No draft loaded')).toBeVisible()
  await expect(page.getByRole('slider')).toHaveCount(0)
  await page.getByRole('button', { name: 'Load simulated example' }).click()
  await expect(page.getByText('Simulated example / inventory.')).toBeVisible()
  await page.screenshot({ path: 'test-results/routing-desktop.png', fullPage: true })
  await gain(page, 'mic to Call gain', '0.37')
  await expect(page.getByRole('slider', { name: 'mic to Record gain', exact: true })).toHaveValue('0.5')
  await page.getByRole('button', { name: 'mic to Call mute', exact: true }).click()
  await expect(page.getByRole('button', { name: 'mic to Record mute', exact: true })).toHaveAttribute('aria-pressed', 'false')
  await gain(page, 'Output call-input gain', '0.23')
  await expect(page.getByRole('slider', { name: 'Output record-input gain', exact: true })).toHaveValue('1')
  await page.getByRole('button', { name: 'Save draft', exact: true }).click()
  await expect(page.getByText('Draft matches stored copy')).toBeVisible()
  expect(api.store.draft!.mediaControls).toEqual(example.mediaControls)
  const downloadEvent = page.waitForEvent('download')
  await page.getByRole('button', { name: 'Export config', exact: true }).click()
  const download = await downloadEvent
  const exported = readConfig(JSON.parse(readFileSync((await download.path())!, 'utf8')))
  expect(exported).toEqual(api.store.draft)
  await page.reload(); await expect(page.getByRole('slider', { name: 'mic to Call gain', exact: true })).toHaveValue('0.37')
  await expect(page.getByRole('button', { name: 'mic to Call mute', exact: true })).toHaveAttribute('aria-pressed', 'true')
  await expect(page.getByRole('slider', { name: 'Output call-input gain', exact: true })).toHaveValue('0.23')
})

test('profile save, rename, load and undo use persisted API state', async ({ page }) => {
  const api = await mockApi(page); await start(page)
  await gain(page, 'mic to Call gain', '0.37')
  await page.getByRole('button', { name: 'Profiles', exact: true }).click()
  await page.getByLabel('Profile name', { exact: true }).fill('Podcast')
  await page.getByRole('button', { name: 'Save new profile', exact: true }).click()
  await expect(page.getByText('Profile saved. Use Save draft', { exact: false })).toBeVisible()
  await page.getByLabel('Saved profile', { exact: true }).selectOption('profile-1')
  await page.getByLabel('Profile name', { exact: true }).fill('Podcast revised')
  await page.getByRole('button', { name: 'Rename profile', exact: true }).click()
  await expect(page.getByRole('option', { name: 'Podcast revised (profile-1)' })).toBeAttached()
  page.once('dialog', dialog => dialog.accept())
  await page.getByRole('button', { name: 'Load into stored draft', exact: true }).click()
  await expect(page.getByText('Draft matches stored copy')).toBeVisible()
  expect(api.store.draft!.sends[0]!.gain).toBe(0.37)
  await page.getByRole('button', { name: 'Undo stored draft', exact: true }).click()
  expect(api.store.draft!.sends[0]!.gain).toBe(1)
  await page.reload()
  await page.getByRole('button', { name: 'Profiles', exact: true }).click()
  await expect(page.getByRole('option', { name: 'Podcast revised (profile-1)' })).toBeAttached()
})

test('conflict refresh preserves local edits, does not retry writes; cancelled reload is safe', async ({ page }) => {
  const api = await mockApi(page); await start(page)
  await gain(page, 'mic to Call gain', '0.37')
  api.conflict()
  await page.getByRole('button', { name: 'Save draft', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('Store conflict')
  await expect(page.getByRole('button', { name: 'Save draft', exact: true })).toBeDisabled()
  api.resolve()
  await page.getByRole('button', { name: 'Refresh', exact: true }).click()
  await expect(page.getByRole('slider', { name: 'mic to Call gain', exact: true })).toHaveValue('0.37')
  expect(api.writes.length).toBe(1)
  page.once('dialog', dialog => dialog.dismiss())
  await page.getByRole('button', { name: 'Reload stored draft', exact: true }).click()
  await expect(page.getByRole('slider', { name: 'mic to Call gain', exact: true })).toHaveValue('0.37')
  page.once('dialog', dialog => dialog.accept())
  await page.getByRole('button', { name: 'Reload stored draft', exact: true }).click()
  await expect(page.getByRole('slider', { name: 'mic to Call gain', exact: true })).toHaveValue('1')
})

test('invalid imports and authoritative validation failures keep the previous config', async ({ page }) => {
  const api = await mockApi(page); await start(page)
  await page.getByText('Import / export JSON', { exact: false }).click()
  await page.getByLabel('Or paste config JSON', { exact: true }).fill('{"schemaVersion":1}')
  await page.getByRole('button', { name: 'Import pasted config', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('schema-2')
  api.rejectValidation()
  await page.getByLabel('Or paste config JSON', { exact: true }).fill(JSON.stringify({ ...example, revision: 8 }))
  await page.getByRole('button', { name: 'Import pasted config', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('Headroom exceeds one')
  await expect(page.getByRole('slider', { name: 'mic to Call gain', exact: true })).toHaveValue('1')
  expect(api.writes).toHaveLength(0)
})

test('narrow layout confines horizontal scroll and range supports keyboard editing', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  await mockApi(page); await start(page)
  const slider = page.getByRole('slider', { name: 'mic to Call gain', exact: true })
  await slider.focus(); await page.keyboard.press('ArrowLeft')
  await expect(slider).toHaveValue('0.99')
  const dimensions = await page.evaluate(() => ({ body: document.documentElement.scrollWidth, viewport: innerWidth, matrix: document.querySelector('.matrix-scroll')!.scrollWidth, region: document.querySelector('.matrix-scroll')!.clientWidth }))
  expect(dimensions.body).toBeLessThanOrEqual(dimensions.viewport)
  expect(dimensions.matrix).toBeGreaterThan(dimensions.region)
  await page.screenshot({ path: 'test-results/routing-mobile.png', fullPage: true })
})

test('inspection shows core planning failure and clears stale report after a send edit', async ({ page }) => {
  await mockApi(page)
  await page.route('**/api/inspect', async route => {
    const { config, inventory } = route.request().postDataJSON()
    expect(config.schemaVersion).toBe(2)
    expect(inventory.evidence).toBe('simulated')
    await route.fulfill({ json: {
      schemaVersion: 1, mode: 'routing-inspection', configRevision: config.revision,
      inventoryRevision: inventory.revision, inventoryEvidence: inventory.evidence,
      nativeState: 'unknown', liveLevels: null, planAvailable: false,
      preflightError: 'Core preflight rejected the imported capacity', plan: null,
      findings: [{ subject: 'preflight', code: 'capacity-exceeded' }],
      sources: [], mixes: [], sends: [], outputs: [], capacity: [],
    } })
  })
  await start(page)
  await page.getByRole('button', { name: 'Load simulated example' }).click()
  await page.getByRole('button', { name: 'Inspect', exact: true }).click()
  await page.getByRole('button', { name: 'Inspect current draft', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Dry-run planning failed' })).toBeVisible()
  await expect(page.getByText('Core preflight rejected the imported capacity', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Routing', exact: true }).click()
  await gain(page, 'mic to Call gain', '0.37')
  await page.getByRole('button', { name: 'Inspect', exact: true }).click()
  await expect(page.getByText('No current inspection.', { exact: false })).toBeVisible()
  await expect(page.getByRole('heading', { name: 'Dry-run planning failed' })).toHaveCount(0)
})

test('offline reconnect keeps local edits and blocks writes until a fresh read', async ({ page }) => {
  await mockApi(page); await start(page)
  await gain(page, 'mic to Call gain', '0.37')
  await page.route('**/api/session', route => route.abort('connectionrefused'))
  await page.getByRole('button', { name: 'Refresh', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('Local service offline')
  await expect(page.getByRole('slider', { name: 'mic to Call gain', exact: true })).toHaveValue('0.37')
  await expect(page.getByRole('button', { name: 'Save draft', exact: true })).toBeDisabled()
  await page.unroute('**/api/session')
  await page.getByRole('button', { name: 'Refresh', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Save draft', exact: true })).toBeEnabled()
  await expect(page.getByRole('slider', { name: 'mic to Call gain', exact: true })).toHaveValue('0.37')
})
