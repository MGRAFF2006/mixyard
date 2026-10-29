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
  await expect(page.locator('.processing > [role="alert"]')).toContainText('exactly 256 interleaved samples')
  expect(api.previews).toHaveLength(0)
  await page.getByLabel('Test signal', { exact: true }).selectOption('sine')
  await page.getByLabel('Sine frequency (Hz)', { exact: true }).fill('123.456')
  api.offline(true)
  await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  await expect(page.locator('.processing > [role="alert"]')).toContainText('Local service offline')
  await expect(page.getByLabel('Sample rate (Hz)', { exact: true })).toHaveValue('4294967295')
  await expect(page.getByRole('button', { name: 'Render and measure', exact: true })).toBeDisabled()
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

test('beginner balance, swap, copy, level and independent delays append explicit operations with exact IDs', async ({ page }) => {
  const api = await mockDsp(page); await open(page)
  await page.getByLabel('Channel 1 ID', { exact: true }).fill(' left / 7 ')
  await page.getByLabel('Channel 2 ID', { exact: true }).fill('aux:ø')
  await page.getByLabel('New channel ID', { exact: true }).fill('rear-3')
  await page.getByRole('button', { name: 'Add channel', exact: true }).click()
  await page.getByRole('button', { name: 'Beginner', exact: true }).click()
  const shortcuts = page.getByRole('group', { name: 'Channel shortcuts', exact: true })
  await shortcuts.getByLabel('Channel level (dB)', { exact: true }).fill('-6')
  await shortcuts.getByLabel('Reverse channel polarity', { exact: true }).check()
  await shortcuts.getByRole('button', { name: 'Add channel operation', exact: true }).click()
  await shortcuts.getByLabel('Operation', { exact: true }).selectOption('balance')
  await shortcuts.getByLabel('First channel / copy from', { exact: true }).selectOption(' left / 7 ')
  await shortcuts.getByLabel('Second channel / copy to', { exact: true }).selectOption('rear-3')
  await shortcuts.getByLabel('Pair balance', { exact: true }).focus()
  for (let step = 0; step < 25; step++) await page.keyboard.press('ArrowRight')
  await shortcuts.getByRole('button', { name: 'Add channel operation', exact: true }).click()
  await shortcuts.getByLabel('Operation', { exact: true }).selectOption('swap')
  await shortcuts.getByRole('button', { name: 'Add channel operation', exact: true }).click()
  await shortcuts.getByLabel('Operation', { exact: true }).selectOption('copy')
  await shortcuts.getByRole('button', { name: 'Add channel operation', exact: true }).click()
  await shortcuts.getByLabel('Operation', { exact: true }).selectOption('delay')
  await shortcuts.getByLabel('Selected channel', { exact: true }).selectOption('aux:ø')
  await shortcuts.getByLabel('Delay (frames)', { exact: true }).fill('48')
  await shortcuts.getByRole('button', { name: 'Add channel operation', exact: true }).click()
  await shortcuts.getByLabel('Selected channel', { exact: true }).selectOption('rear-3')
  await shortcuts.getByLabel('Delay (frames)', { exact: true }).fill('96')
  await shortcuts.getByRole('button', { name: 'Add channel operation', exact: true }).click()
  expect(api.previews).toHaveLength(0)
  api.slow(); await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('Slower than')
  const config = api.previews[0]!.config
  expect(config.format.channels).toEqual([' left / 7 ', 'aux:ø', 'rear-3'])
  expect(config.effects.map(item => ({ channels: item.channels, effect: item.effect }))).toEqual([
    { channels: [' left / 7 '], effect: { kind: 'gain', gain: 10 ** (-6 / 20), invertPolarity: true } },
    { channels: undefined, effect: { kind: 'channel-matrix', routes: [
      { output: ' left / 7 ', inputs: [{ channel: ' left / 7 ', gain: 0.75 }] }, { output: 'aux:ø', inputs: [{ channel: 'aux:ø', gain: 1 }] }, { output: 'rear-3', inputs: [{ channel: 'rear-3', gain: 1 }] },
    ] } },
    { channels: undefined, effect: { kind: 'channel-matrix', routes: [
      { output: ' left / 7 ', inputs: [{ channel: 'rear-3', gain: 1 }] }, { output: 'aux:ø', inputs: [{ channel: 'aux:ø', gain: 1 }] }, { output: 'rear-3', inputs: [{ channel: ' left / 7 ', gain: 1 }] },
    ] } },
    { channels: undefined, effect: { kind: 'channel-matrix', routes: [
      { output: ' left / 7 ', inputs: [{ channel: ' left / 7 ', gain: 1 }] }, { output: 'aux:ø', inputs: [{ channel: 'aux:ø', gain: 1 }] }, { output: 'rear-3', inputs: [{ channel: ' left / 7 ', gain: 1 }] },
    ] } },
    { channels: ['aux:ø'], effect: { kind: 'delay', frames: 48, wet: 1, dry: 0, feedback: 0 } },
    { channels: ['rear-3'], effect: { kind: 'delay', frames: 96, wet: 1, dry: 0, feedback: 0 } },
  ])
  await expect(shortcuts.getByRole('button', { name: 'Add channel operation', exact: true })).toBeEnabled()
  await page.screenshot({ path: 'test-results/channel-beginner-desktop.png', fullPage: true })
})

test('every original effect targets explicit independent channels and different ordered dynamics retain separate values', async ({ page }) => {
  const api = await mockDsp(page); await open(page)
  const imported = starter('empty'); imported.format.channels = ['first / 7', '  second  ', 'aux']
  await paste(page, imported)
  for (const kind of effectKinds.filter(kind => kind !== 'channel-matrix')) {
    await add(page, kind)
    const stage = page.locator('.dsp-effect').last()
    await stage.getByLabel('Apply effect to', { exact: true }).selectOption('selected')
    await stage.getByRole('checkbox', { name: '"  second  "', exact: true }).check()
    await stage.getByRole('checkbox', { name: '"first / 7"', exact: true }).uncheck()
  }
  await page.getByRole('group', { name: '4. compressor', exact: true }).getByLabel('Threshold (dB)', { exact: true }).fill('-22.5')
  await add(page, 'compressor')
  const other = page.getByRole('group', { name: '7. compressor', exact: true })
  await other.getByLabel('Apply effect to', { exact: true }).selectOption('selected')
  await other.getByLabel('Threshold (dB)', { exact: true }).fill('-12.25')
  await other.getByLabel('Attack (ms)', { exact: true }).fill('11')
  await page.getByRole('button', { name: 'Move channel 2 up', exact: true }).click()
  await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Float64 block output' })).toBeVisible()
  expect(api.previews[0]!.config.format.channels).toEqual(['  second  ', 'first / 7', 'aux'])
  expect(api.previews[0]!.config.effects.map(item => item.channels)).toEqual([['  second  '], ['  second  '], ['  second  '], ['  second  '], ['  second  '], ['  second  '], ['first / 7']])
  expect(api.previews[0]!.config.effects[3]!.effect).toMatchObject({ parameters: { thresholdDb: -22.5, attackMs: 5 } })
  expect(api.previews[0]!.config.effects[6]!.effect).toMatchObject({ parameters: { thresholdDb: -12.25, attackMs: 11 } })
  await other.getByRole('checkbox', { name: '"first / 7"', exact: true }).uncheck()
  await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('nonempty, unique, known')
  expect(api.previews).toHaveLength(1)
})

test('custom matrices, exact coefficients and raw fields survive all detail views, export and compile-only reimport', async ({ page }) => {
  const { dspJson } = await import('../src/dsp-json.ts')
  const api = await mockDsp(page); await open(page)
  const config = starter('fir'); config.format.channels = ['in / 7', ' aux ', 'rear']
  config.effects[0]!.channels = [' aux ']; config.effects[0]!.effect = { kind: 'fir', coefficients: [-0, 1.0000000000000002] }
  config.effects.push({ name: 'Imported sum', enabled: false, raw: { retained: true }, effect: { kind: 'channel-matrix', routes: [
    { output: 'in / 7', extra: -0, inputs: [{ channel: ' aux ', gain: -1.0000000000000002 }, { channel: 'rear', gain: -0, extra: 'kept' }] },
    { output: ' aux ', inputs: [] }, { output: 'rear', inputs: [{ channel: 'in / 7', gain: 1e100 }] },
  ] } })
  await page.getByText('Import chain JSON', { exact: true }).click()
  await page.getByLabel('Or paste chain JSON', { exact: true }).fill(dspJson(config))
  await page.getByRole('button', { name: 'Validate and import chain', exact: true }).click()
  await expect(page.getByText('Chain imported; core validation passed.', { exact: false })).toBeVisible()
  for (const view of ['Beginner', 'Expert', 'Everyday']) {
    await page.getByRole('button', { name: view, exact: true }).click()
    if (view !== 'Beginner') await expect(page.getByText('Custom matrix — coefficients preserved', { exact: true })).toBeVisible()
    const event = page.waitForEvent('download')
    await page.getByRole('button', { name: 'Export chain', exact: true }).click()
    const download = await event
    const text = readFileSync((await download.path())!, 'utf8')
    expect(JSON.parse(text)).toEqual(config)
    expect(text).toContain('"gain": -0')
  }
  expect(api.previews).toHaveLength(0); expect(api.validations).toHaveLength(1)
  await page.getByRole('button', { name: 'Expert', exact: true }).click()
  await page.screenshot({ path: 'test-results/channel-expert-desktop.png', fullPage: true })
  const event = page.waitForEvent('download')
  await page.getByRole('button', { name: 'Export chain', exact: true }).click()
  const exported = readFileSync((await (await event).path())!, 'utf8')
  await page.getByLabel('Or paste chain JSON', { exact: true }).fill(exported)
  await page.getByRole('button', { name: 'Validate and import chain', exact: true }).click()
  await expect(page.getByText('Chain imported; core validation passed.', { exact: false })).toBeVisible()
  expect(api.validations).toHaveLength(2)
  await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Float64 block output' })).toBeVisible()
  expect(api.previews[0]!.config).toEqual(config)
})

test('format edits preserve same-identity references and block removal/expansion; bad imported references never reach core', async ({ page }) => {
  const api = await mockDsp(page); await open(page)
  const c = starter('empty'); c.format.channels = ['right / 7', ' left ']
  c.effects = [{ ...newEffect('gain'), channels: ['right / 7'] }, newEffect('channel-matrix', c.format.channels)]
  await paste(page, c)
  await page.getByLabel('Channel 1 ID', { exact: true }).fill('renamed:ø')
  await page.getByRole('button', { name: 'Move channel 1 down', exact: true }).click()
  await page.getByRole('button', { name: 'Remove channel 2', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('referenced')
  await page.getByLabel('New channel ID', { exact: true }).fill('new')
  await page.getByRole('button', { name: 'Add channel', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('fixed format')
  await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Float64 block output' })).toBeVisible()
  const rendered = api.previews[0]!.config
  expect(rendered.format.channels).toEqual([' left ', 'renamed:ø'])
  expect(rendered.effects[0]!.channels).toEqual(['renamed:ø'])
  expect(rendered.effects[1]!.effect).toEqual({ kind: 'channel-matrix', routes: [
    { output: 'renamed:ø', inputs: [{ channel: 'renamed:ø', gain: 1 }] }, { output: ' left ', inputs: [{ channel: ' left ', gain: 1 }] },
  ] })
  c.effects[0]!.channels = ['unknown']
  await page.getByLabel('Or paste chain JSON', { exact: true }).fill(JSON.stringify(c))
  await page.getByRole('button', { name: 'Validate and import chain', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('known channel IDs')
  expect(api.validations).toHaveLength(1)
  await expect(page.getByLabel('Channel 2 ID', { exact: true })).toHaveValue('renamed:ø')
})

test('narrow keyboard alignment explicitly quantizes milliseconds and retains independent sample delays', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  const api = await mockDsp(page); await open(page)
  await page.getByLabel('Sample rate (Hz)', { exact: true }).fill('44100')
  await page.getByRole('button', { name: 'Beginner', exact: true }).click()
  const shortcuts = page.getByRole('group', { name: 'Channel shortcuts', exact: true })
  await shortcuts.getByLabel('Operation', { exact: true }).selectOption('delay')
  await shortcuts.getByText('Set delay in milliseconds', { exact: true }).click()
  await shortcuts.getByLabel('Requested delay (ms)', { exact: true }).fill('1')
  await expect(shortcuts.getByRole('button', { name: 'Apply milliseconds', exact: true })).toBeDisabled()
  await shortcuts.getByLabel('Sample quantization', { exact: true }).selectOption('nearest')
  await expect(shortcuts.getByText('Apply 44 samples', { exact: false })).toBeVisible()
  await shortcuts.getByRole('button', { name: 'Apply milliseconds', exact: true }).focus(); await page.keyboard.press('Enter')
  await expect(shortcuts.getByLabel('Delay (frames)', { exact: true })).toHaveValue('44')
  await shortcuts.getByRole('button', { name: 'Add channel operation', exact: true }).focus(); await page.keyboard.press('Enter')
  await shortcuts.getByLabel('Selected channel', { exact: true }).selectOption('FR')
  await shortcuts.getByLabel('Delay (frames)', { exact: true }).fill('88')
  await shortcuts.getByRole('button', { name: 'Add channel operation', exact: true }).click()
  await page.getByRole('button', { name: 'Everyday', exact: true }).click()
  await page.getByRole('group', { name: '1. delay', exact: true }).getByLabel('Delay (frames)', { exact: true }).focus()
  await page.keyboard.press('ArrowUp')
  await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Float64 block output' })).toBeVisible()
  expect(api.previews[0]!.config.effects.map(e => ({ channels: e.channels, effect: e.effect }))).toEqual([
    { channels: ['FL'], effect: { kind: 'delay', frames: 45, wet: 1, dry: 0, feedback: 0 } },
    { channels: ['FR'], effect: { kind: 'delay', frames: 88, wet: 1, dry: 0, feedback: 0 } },
  ])
  const dimensions = await page.evaluate(() => ({ body: document.documentElement.scrollWidth, viewport: innerWidth }))
  expect(dimensions.body).toBeLessThanOrEqual(dimensions.viewport)
  await page.getByRole('button', { name: 'Beginner', exact: true }).click()
  await page.screenshot({ path: 'test-results/channel-mobile.png', fullPage: true })
})
