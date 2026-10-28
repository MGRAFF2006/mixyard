import { readFileSync } from 'node:fs'
import { test, expect } from '@playwright/test'
import type { Page } from '@playwright/test'
import { effectKinds, filters, newEffect, starter } from '../src/dsp.ts'
import type { PreviewRequest } from '../src/dsp.ts'
import { admission, previewFixture } from './dsp-fixture.ts'

async function mockDsp(page: Page) {
  const previews: PreviewRequest[] = [], validations: unknown[] = [], writes: string[] = []
  let slow = false, offline = false, invalid = false, measured = true
  await page.route('**/api/**', async route => {
    const path = new URL(route.request().url()).pathname
    const reply = (json: unknown, status = 200) => route.fulfill({ status, json })
    if (path === '/api/session') return reply({ apiVersion: 1, csrfToken: 'dsp-session', audioAvailable: false })
    expect(route.request().headers()['x-mixyard-token']).toBe('dsp-session')
    if (path === '/api/profiles/read') return reply({ format: 'mixyard-local-drafts', schemaVersion: 1, storeRevision: 0, configRevision: 0, profiles: [], draft: null, previousDraft: null })
    if (path === '/api/status') return reply({ connected: false, snapshot: null, error: null })
    if (path === '/api/dsp/validate') {
      validations.push(route.request().postDataJSON())
      if (invalid) return reply({ error: { code: 'invalid-parameter', message: 'Core rejected filter frequency above Nyquist' } }, 422)
      return reply({ valid: true, precision: 'float64', admission })
    }
    if (path === '/api/dsp/preview') {
      const body = route.request().postDataJSON() as PreviewRequest; previews.push(body)
      if (offline) return route.abort('connectionrefused')
      return reply(previewFixture(body, { slow, measured }))
    }
    writes.push(path); return reply({ error: { code: 'unexpected', message: path } }, 404)
  })
  return { previews, validations, writes, slow() { slow = true }, offline(value: boolean) { offline = value }, invalid(value: boolean) { invalid = value }, unmeasured() { measured = false } }
}
async function open(page: Page) {
  await page.goto('/'); await expect(page.getByRole('button', { name: 'Refresh', exact: true })).toBeEnabled()
  await page.getByRole('button', { name: 'Processing', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Processing workbench' })).toBeVisible()
}
async function add(page: Page, kind: string) { await page.getByLabel('New effect', { exact: true }).selectOption(kind); await page.getByRole('button', { name: 'Add effect', exact: true }).click() }
async function paste(page: Page, value: unknown) {
  await page.getByText('Import chain JSON', { exact: true }).click()
  await page.getByLabel('Or paste chain JSON', { exact: true }).fill(JSON.stringify(value))
  await page.getByRole('button', { name: 'Validate and import chain', exact: true }).click()
}

test('ordered editor sends exact Float64 request only on Render; slow result keeps editing and offline render usable', async ({ page }) => {
  const api = await mockDsp(page); await open(page)
  await page.getByLabel('Chain name', { exact: true }).fill('Precision chain')
  await page.getByLabel('Sample rate (Hz)', { exact: true }).fill('123457')
  await page.getByLabel('Channel 1 ID', { exact: true }).fill(' right ')
  await page.getByLabel('Channel 2 ID', { exact: true }).fill('aux')
  await page.getByRole('button', { name: 'Move channel 2 up', exact: true }).click()
  await add(page, 'gain'); await page.getByLabel('Linear gain', { exact: true }).fill('1.0000000000000002')
  await page.getByLabel('Invert polarity', { exact: true }).check()
  await add(page, 'delay'); await page.getByLabel('Feedback', { exact: true }).fill('-0.3')
  await page.getByRole('button', { name: 'Move effect 2 up', exact: true }).click()
  const delay = page.getByRole('group', { name: '1. delay', exact: true })
  await delay.getByLabel('Enabled', { exact: true }).uncheck()
  await page.getByLabel('Bypass whole chain', { exact: true }).check()
  await page.getByLabel('Block frames', { exact: true }).fill('2')
  await page.getByLabel('Timing repetitions', { exact: true }).fill('3')
  await page.getByLabel('Test signal', { exact: true }).selectOption('samples')
  await page.getByLabel('Input samples (JSON array)', { exact: true }).fill('[-0,1.0000000000000002,-2,1e100]')
  await page.getByLabel('Chain name', { exact: true }).focus(); await page.keyboard.press('Enter')
  expect(api.previews).toHaveLength(0); expect(api.validations).toHaveLength(0)
  api.slow(); await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('Slower than the observed live block budget')
  const gain = newEffect('gain'); gain.effect = { kind: 'gain', gain: 1.0000000000000002, invertPolarity: true }
  const echo = newEffect('delay'); echo.enabled = false
  if (echo.effect.kind === 'delay') echo.effect.feedback = -0.3
  expect(api.previews).toEqual([{ config: { name: 'Precision chain', format: { sampleRateHz: 123457, channels: ['aux', ' right '] }, effects: [echo, gain] }, frames: 2, iterations: 3, bypassed: true, signal: { kind: 'samples', samples: [-0, 1.0000000000000002, -2, 1e100] } }])
  await expect(page.getByText('4 output samples exceed full scale', { exact: false })).toBeVisible()
  await expect(page.getByText('mock-release / mock-linux / mock-x86_64', { exact: false })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Render and measure', exact: true })).toBeEnabled()
  await expect(page.getByRole('button', { name: 'Add effect', exact: true })).toBeEnabled()
  await expect(delay.getByLabel('Enabled', { exact: true })).not.toBeChecked()
  await expect(page.getByLabel('Bypass whole chain', { exact: true })).toBeChecked()
  await page.screenshot({ path: 'test-results/processing-desktop.png', fullPage: true })
  await page.getByLabel('Linear gain', { exact: true }).fill('2')
  await expect(page.getByText('Speed unknown. No current preview or timing.', { exact: false })).toBeVisible()
  await expect(page.getByRole('heading', { name: 'Float64 block output' })).toHaveCount(0)
  await page.getByRole('button', { name: 'Routing', exact: true }).click()
  await page.getByRole('button', { name: 'Processing', exact: true }).click()
  await expect(page.getByLabel('Linear gain', { exact: true })).toHaveValue('2')
  expect(api.writes).toEqual([])
})

test('every effect has usable controls, all biquads are selectable and FIR data is never shortened', async ({ page }) => {
  const api = await mockDsp(page); await open(page)
  for (const kind of effectKinds) await add(page, kind)
  const biquad = page.getByRole('group', { name: '2. biquad', exact: true })
  for (const filter of filters) {
    await biquad.getByLabel('Filter', { exact: true }).selectOption(filter)
    await expect(biquad.getByLabel('Filter', { exact: true })).toHaveValue(filter)
  }
  await biquad.getByLabel('Frequency (Hz)', { exact: true }).fill('321.1234567890123')
  await biquad.getByLabel('Q', { exact: true }).fill('1.23456789012345')
  await biquad.getByLabel('Filter gain (dB)', { exact: true }).fill('0')
  const compressor = page.getByRole('group', { name: '4. compressor', exact: true })
  const expander = page.getByRole('group', { name: '5. expander', exact: true })
  for (const group of [compressor, expander]) {
    await group.getByLabel('Threshold (dB)', { exact: true }).fill('-22.25')
    await group.getByLabel('Ratio', { exact: true }).fill('4')
    await group.getByLabel('Knee (dB)', { exact: true }).fill('5')
    await group.getByLabel('Attack (ms)', { exact: true }).fill('0')
    await group.getByLabel('Release (ms)', { exact: true }).fill('120')
    await group.getByLabel('Makeup (dB)', { exact: true }).fill('1.25')
    await group.getByLabel('Range (dB)', { exact: true }).fill('90')
  }
  const coefficients = [1.0000000000000002, ...Array(8192).fill(0), -0.125]
  await page.getByLabel('FIR coefficients (JSON array)', { exact: true }).fill(JSON.stringify(coefficients))
  await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Float64 block output' })).toBeVisible()
  const body = api.previews[0]!
  expect(body.config.effects[5]!.effect).toEqual({ kind: 'fir', coefficients })
  expect(body.config.effects[3]!.effect).toEqual({ kind: 'compressor', parameters: { thresholdDb: -22.25, ratio: 4, kneeDb: 5, attackMs: 0, releaseMs: 120, makeupDb: 1.25, rangeDb: 90 } })
  await page.getByRole('button', { name: 'Remove effect 3', exact: true }).click()
  await expect(page.getByLabel('Delay (frames)', { exact: true })).toHaveCount(0)
  await expect(page.getByText('Speed unknown. No current preview', { exact: false })).toBeVisible()
})

test('imports validate without rendering, preserve raw fields/IDs on export, and failures keep prior chain', async ({ page }) => {
  const api = await mockDsp(page); await open(page)
  const imported = starter('fir'); imported.name = 'Imported'; imported.format.channels = ['back/right', ' left ']; imported.format.sampleRateHz = 17
  imported.extraRaw = { kept: true }; imported.effects[0]!.extraRaw = [1, 2, 3]
  await paste(page, imported)
  await expect(page.getByLabel('Chain name', { exact: true })).toHaveValue('Imported')
  expect(api.previews).toHaveLength(0)
  expect(api.validations).toEqual([{ config: imported, frames: 1 }])
  const downloadEvent = page.waitForEvent('download')
  await page.getByRole('button', { name: 'Export chain', exact: true }).click()
  const download = await downloadEvent
  expect(JSON.parse(readFileSync((await download.path())!, 'utf8'))).toEqual(imported)
  await page.getByLabel('Or paste chain JSON', { exact: true }).fill('{"name":"invalid"}')
  await page.getByRole('button', { name: 'Validate and import chain', exact: true }).click()
  await expect(page.getByRole('alert')).toBeVisible()
  await expect(page.getByLabel('Chain name', { exact: true })).toHaveValue('Imported')
  expect(api.validations).toHaveLength(1)
  api.invalid(true)
  await page.getByLabel('Or paste chain JSON', { exact: true }).fill(JSON.stringify(starter('echo')))
  await page.getByRole('button', { name: 'Validate and import chain', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('Core rejected filter frequency')
  await expect(page.getByLabel('Chain name', { exact: true })).toHaveValue('Imported')
  api.invalid(false)
  await page.getByLabel('Chain JSON file', { exact: true }).setInputFiles({ name: 'chain.json', mimeType: 'application/json', buffer: Buffer.from(JSON.stringify(starter('trim'))) })
  await expect(page.getByLabel('Chain name', { exact: true })).toHaveValue('Half-amplitude trim')
  expect(api.previews).toHaveLength(0); expect(api.writes).toHaveLength(0)
})

test('invalid numeric/array/custom input cannot render; offline errors allow recovery with edits preserved', async ({ page }) => {
  const api = await mockDsp(page); await open(page)
  await add(page, 'fir')
  await page.getByLabel('FIR coefficients (JSON array)', { exact: true }).fill('[1, "bad"]')
  await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  expect(api.previews).toHaveLength(0)
  await page.getByLabel('FIR coefficients (JSON array)', { exact: true }).fill('[1, 0.000000000000000123]')
  await page.getByLabel('Sample rate (Hz)', { exact: true }).fill('')
  await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  expect(api.previews).toHaveLength(0)
  await page.getByLabel('Sample rate (Hz)', { exact: true }).fill('4294967295')
  await page.getByLabel('Test signal', { exact: true }).selectOption('samples')
  await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('exactly 256 interleaved samples')
  expect(api.previews).toHaveLength(0)
  await page.getByLabel('Test signal', { exact: true }).selectOption('sine')
  await page.getByLabel('Sine frequency (Hz)', { exact: true }).fill('123.456')
  api.offline(true)
  await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('Local service offline')
  await expect(page.getByLabel('Sample rate (Hz)', { exact: true })).toHaveValue('4294967295')
  await expect(page.getByRole('button', { name: 'Render and measure', exact: true })).toBeEnabled()
  api.offline(false)
  await page.getByRole('button', { name: 'Refresh', exact: true }).click()
  await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Float64 block output' })).toBeVisible()
  expect(api.previews.at(-1)!.signal).toEqual({ kind: 'sine', amplitude: 0.25, frequencyHz: 123.456 })
  await page.getByLabel('Timing repetitions', { exact: true }).fill('4')
  await expect(page.getByText('Speed unknown. No current preview', { exact: false })).toBeVisible()
})

test('narrow workbench supports keyboard ordering/editing and unknown timing is not a pass', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  const api = await mockDsp(page); api.unmeasured(); await open(page)
  await page.getByLabel('Starter preset', { exact: true }).selectOption('echo')
  await page.getByRole('button', { name: 'Load starter (replaces chain)', exact: true }).click()
  const delay = page.getByLabel('Delay (frames)', { exact: true })
  await delay.focus(); await page.keyboard.press('ArrowUp'); await expect(delay).toHaveValue('961')
  await add(page, 'gain')
  await page.getByRole('button', { name: 'Move effect 2 up', exact: true }).focus()
  await page.keyboard.press('Enter')
  await expect(page.getByRole('group', { name: '1. gain', exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  await expect(page.getByText('Speed unknown: the timer', { exact: false })).toBeVisible()
  await expect(page.getByText('Observed blocks were within', { exact: false })).toHaveCount(0)
  const dimensions = await page.evaluate(() => ({ body: document.documentElement.scrollWidth, viewport: innerWidth }))
  expect(dimensions.body).toBeLessThanOrEqual(dimensions.viewport)
  await page.screenshot({ path: 'test-results/processing-mobile.png', fullPage: true })
  await page.getByRole('button', { name: 'Remove channel 2', exact: true }).click()
  await expect(page.getByLabel('Channel 2 ID', { exact: true })).toHaveCount(0)
})
