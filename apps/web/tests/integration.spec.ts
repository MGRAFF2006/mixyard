import { spawn, spawnSync } from 'node:child_process'
import type { ChildProcess } from 'node:child_process'
import { once } from 'node:events'
import { mkdtempSync, readFileSync, realpathSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { createInterface } from 'node:readline'
import { fileURLToPath } from 'node:url'
import { test as base, expect } from '@playwright/test'
import type { Page } from '@playwright/test'

const root = fileURLToPath(new URL('../../../', import.meta.url))
const suffix = process.platform === 'win32' ? '.exe' : ''
const binary = resolve(process.env.MIXYARD_WEB_BINARY ?? join(root, 'target/debug', `mixyard-web${suffix}`))
const runtimeBinary = join(dirname(binary), `mixyard-runtime${suffix}`)
const cliBinary = join(dirname(binary), `mixyard${suffix}`)

class Service {
  directory = realpathSync(mkdtempSync(join(tmpdir(), 'mixyard-web-browser-')))
  instance = `webtest-${process.pid}-${Date.now().toString(36)}`
  url = ''
  process: ChildProcess | null = null
  async start() {
    const port = this.url ? new URL(this.url).port : '0'
    const child = spawn(binary, ['--assets', join(root, 'apps/web/dist'), '--store', join(this.directory, 'store'), '--port', port, '--instance', this.instance], { stdio: ['ignore', 'pipe', 'inherit'] })
    this.process = child
    const lines = createInterface({ input: child.stdout! })
    let timer: ReturnType<typeof setTimeout> | undefined
    try {
      const line = await Promise.race([
        once(lines, 'line').then(([line]) => String(line)),
        once(child, 'exit').then(([code]) => { throw new Error(`Web service exited during startup: ${code}`) }),
        once(child, 'error').then(([error]) => { throw error }),
        new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error('Web service startup timed out. Build the binaries first.')), 10_000) }),
      ])
      expect(line).toMatch(/^Mixyard web: http:\/\/127\.0\.0\.1:\d+$/)
      this.url = line.slice('Mixyard web: '.length)
    } finally { clearTimeout(timer); lines.close() }
  }
  async stop() {
    const child = this.process
    this.process = null
    if (child && child.exitCode === null && child.signalCode === null) {
      const exited = once(child, 'exit')
      child.kill()
      await exited
    }
  }
  async post(path: string, body = {}) {
    const session = await fetch(`${this.url}/api/session`).then(response => response.json())
    const response = await fetch(this.url + path, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Mixyard-Token': session.csrfToken }, body: JSON.stringify(body) })
    expect(response.status).toBe(200)
    return response.json()
  }
  stored() { return JSON.parse(readFileSync(join(this.directory, 'store/profiles.json'), 'utf8')) }
}

const test = base.extend<{ service: Service }>({
  service: async ({}, use) => {
    const service = new Service()
    try { await service.start(); await use(service) }
    finally { await service.stop(); rmSync(service.directory, { recursive: true, force: true }) }
  },
})
async function open(page: Page, service: Service) {
  await page.goto(service.url)
  await expect(page.getByRole('button', { name: 'Refresh', exact: true })).toBeEnabled()
}
async function example(page: Page) {
  await page.getByRole('button', { name: 'Load simulated example' }).click()
  await expect(page.getByRole('slider', { name: 'mic to Call gain', exact: true })).toHaveValue('1')
}
async function save(page: Page) {
  await page.getByRole('button', { name: 'Save draft', exact: true }).click()
  await expect(page.getByText('Draft matches stored copy', { exact: true })).toBeVisible()
}

test('production assets and actual store preserve independent edits through browser and service restart', async ({ page, service }) => {
  const cspErrors: string[] = []
  page.on('console', message => { if (/content.security.policy|refused to (execute|apply|load)/i.test(message.text())) cspErrors.push(message.text()) })
  await open(page, service)
  await expect(page.getByText('No draft loaded', { exact: true })).toBeVisible()
  await expect(page.getByRole('slider')).toHaveCount(0)
  await example(page)
  await expect(page.getByRole('button', { name: 'Add return to Call send at zero gain', exact: true })).toHaveCount(0)
  await expect(page.getByText('Call return excluded', { exact: true })).toBeVisible()
  await page.getByRole('slider', { name: 'mic to Call gain', exact: true }).fill('0.37')
  await page.getByRole('button', { name: 'mic to Call mute', exact: true }).click()
  await page.getByRole('slider', { name: 'Output call-input gain', exact: true }).fill('0.23')
  await save(page)
  const stored = service.stored()
  expect(stored.draft.sends.find((send: { source: string; mix: string }) => send.source === 'mic' && send.mix === 'call')).toMatchObject({ gain: 0.37, mute: true })
  expect(stored.draft.sends.find((send: { source: string; mix: string }) => send.source === 'mic' && send.mix === 'record').gain).toBe(0.5)
  await service.stop(); await service.start()
  await page.reload()
  await expect(page.getByRole('slider', { name: 'mic to Call gain', exact: true })).toHaveValue('0.37')
  await expect(page.getByRole('button', { name: 'mic to Call mute', exact: true })).toHaveAttribute('aria-pressed', 'true')
  await expect(page.getByRole('slider', { name: 'Output call-input gain', exact: true })).toHaveValue('0.23')
  await expect(page.getByRole('slider', { name: 'Output record-input gain', exact: true })).toHaveValue('1')
  expect(cspErrors).toEqual([])
  await page.screenshot({ path: 'test-results/integration-routing-desktop.png', fullPage: true })
})

test('actual profiles save, rename, load and undo; competing browser writes preserve local edits', async ({ page, context, service }) => {
  await open(page, service); await example(page); await save(page)
  await page.getByRole('slider', { name: 'mic to Call gain', exact: true }).fill('0.37')
  await page.getByRole('button', { name: 'Profiles', exact: true }).click()
  await page.getByLabel('Profile name', { exact: true }).fill('Podcast')
  await page.getByRole('button', { name: 'Save new profile', exact: true }).click()
  await expect(page.getByText('Profile saved. Use Save draft', { exact: false })).toBeVisible()
  const id = service.stored().profiles[0].id
  await page.getByLabel('Saved profile', { exact: true }).selectOption(id)
  await page.getByLabel('Profile name', { exact: true }).fill('Podcast revised')
  await page.getByRole('button', { name: 'Rename profile', exact: true }).click()
  await expect(page.getByRole('option', { name: `Podcast revised (${id})` })).toBeAttached()
  page.once('dialog', dialog => dialog.accept())
  await page.getByRole('button', { name: 'Load into stored draft', exact: true }).click()
  await expect(page.getByText('Draft matches stored copy', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Undo stored draft', exact: true }).click()
  await expect(page.getByText('Previous stored draft restored.', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Routing', exact: true }).click()
  await expect(page.getByRole('slider', { name: 'mic to Call gain', exact: true })).toHaveValue('1')
  const second = await context.newPage()
  await open(second, service)
  await second.getByRole('slider', { name: 'mic to Call gain', exact: true }).fill('0.33')
  await page.getByRole('slider', { name: 'mic to Call gain', exact: true }).fill('0.24'); await save(page)
  await second.getByRole('button', { name: 'Save draft', exact: true }).click()
  await expect(second.getByRole('alert')).toContainText('Store conflict')
  await second.getByRole('button', { name: 'Refresh', exact: true }).click()
  await expect(second.getByRole('slider', { name: 'mic to Call gain', exact: true })).toHaveValue('0.33')
  expect(service.stored().draft.sends[0].gain).toBe(0.24)
  await second.close({ runBeforeUnload: false })
})

test('real diagnostics, invalid imports, offline recovery and narrow keyboard controls', async ({ page, service }) => {
  await open(page, service); await example(page)
  await page.getByRole('button', { name: 'Inspect', exact: true }).click()
  await page.getByRole('button', { name: 'Inspect current draft', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Dry-run plan available' })).toBeVisible()
  await expect(page.getByText('Planned coefficients are not measured audio.', { exact: false })).toBeVisible()
  await page.getByText('Import / export JSON', { exact: false }).click()
  await page.getByLabel('Or paste config JSON', { exact: true }).fill('{"schemaVersion":1}')
  await page.getByRole('button', { name: 'Import pasted config', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('schema-2')
  await page.getByRole('button', { name: 'Routing', exact: true }).click()
  await page.setViewportSize({ width: 390, height: 844 })
  const slider = page.getByRole('slider', { name: 'mic to Call gain', exact: true })
  await slider.focus(); await page.keyboard.press('ArrowLeft'); await expect(slider).toHaveValue('0.99')
  const dimensions = await page.evaluate(() => ({ body: document.documentElement.scrollWidth, width: innerWidth }))
  expect(dimensions.body).toBeLessThanOrEqual(dimensions.width)
  await page.screenshot({ path: 'test-results/integration-routing-mobile.png', fullPage: true })
  await service.stop()
  await page.getByRole('button', { name: 'Refresh', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('Local service offline')
  await expect(slider).toHaveValue('0.99')
  await service.start()
  await page.getByRole('button', { name: 'Refresh', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Save draft', exact: true })).toBeEnabled()
  await expect(slider).toHaveValue('0.99')
})

test('runtime connection remains read only and UI closure does not stop its owned runtime', async ({ page, service }) => {
  await open(page, service)
  await expect(page.getByText('Runtime disconnected', { exact: true })).toBeVisible()
  const runtime = spawn(runtimeBinary, ['serve', '--instance', service.instance], { stdio: 'ignore' })
  try {
    await expect.poll(async () => (await service.post('/api/status')).connected).toBe(true)
    await page.getByRole('button', { name: 'Refresh', exact: true }).click()
    await expect(page.getByText('Runtime connected · read only', { exact: true })).toBeVisible()
    const status = await service.post('/api/status')
    expect(status.snapshot.availability).toMatchObject({ backendAvailable: false, audioRevision: null, appliedRevision: null, desiredRevision: null })
    await page.close()
    await service.stop(); await service.start()
    expect((await service.post('/api/status')).connected).toBe(true)
    expect(runtime.exitCode).toBeNull()
  } finally {
    const shutdown = spawnSync(cliBinary, ['shutdown', '--instance', service.instance], { encoding: 'utf8', timeout: 10_000 })
    if (shutdown.status !== 0) runtime.kill()
    if (runtime.exitCode === null && runtime.signalCode === null) await once(runtime, 'exit')
  }
})

test('processing editor renders real Float64 bypass and repaired extreme dynamics', async ({ page, service }) => {
  await open(page, service)
  await page.getByRole('button', { name: 'Processing', exact: true }).click()
  await expect(page.getByText('Speed unknown. No current preview or timing.', { exact: false })).toBeVisible()
  await page.getByLabel('New effect', { exact: true }).selectOption('gain')
  await page.getByRole('button', { name: 'Add effect', exact: true }).click()
  await page.getByLabel('Linear gain', { exact: true }).fill('2')
  await page.getByLabel('Bypass whole chain', { exact: true }).check()
  await page.getByLabel('Block frames', { exact: true }).fill('2')
  await page.getByLabel('Timing repetitions', { exact: true }).fill('2')
  await page.getByLabel('Test signal', { exact: true }).selectOption('samples')
  await page.getByLabel('Input samples (JSON array)', { exact: true }).fill('[-0,1.0000000000000002,0.9554529902278269,0.9932393975813079]')
  const response = page.waitForResponse(value => value.url().endsWith('/api/dsp/preview'))
  await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  const rendered = await (await response).json()
  await expect(page.getByRole('heading', { name: 'Float64 block output', exact: true })).toBeVisible()
  expect(rendered.output).toEqual([-0, 1.0000000000000002, 0.9554529902278269, 0.9932393975813079])
  expect(Object.is(rendered.output[0], -0)).toBe(true)
  expect(rendered.report.bypassed).toBe(true)
  await page.getByLabel('Bypass whole chain', { exact: true }).uncheck()
  await page.getByLabel('Linear gain', { exact: true }).fill('0.9554529902278269')
  await page.getByLabel('Block frames', { exact: true }).fill('1')
  await page.getByLabel('Input samples (JSON array)', { exact: true }).fill('[1,1]')
  const gainResponse = page.waitForResponse(value => value.url().endsWith('/api/dsp/preview'))
  await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  expect((await (await gainResponse).json()).output).toEqual([0.9554529902278269, 0.9554529902278269])
  await page.getByRole('button', { name: 'Remove effect 1', exact: true }).click()
  await page.getByLabel('New effect', { exact: true }).selectOption('compressor')
  await page.getByRole('button', { name: 'Add effect', exact: true }).click()
  for (const [name, value] of [['Threshold (dB)', '-6000'], ['Ratio', '4'], ['Knee (dB)', '0'], ['Attack (ms)', '0'], ['Release (ms)', '0'], ['Makeup (dB)', '0']] as const) {
    await page.getByLabel(name, { exact: true }).fill(value)
  }
  await page.getByLabel('Block frames', { exact: true }).fill('1')
  await page.getByLabel('Input samples (JSON array)', { exact: true }).fill('[1e300,-1e300]')
  const dynamics = page.waitForResponse(value => value.url().endsWith('/api/dsp/preview'))
  await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  const result = await (await dynamics).json()
  expect(result.output[0] / 1e-150).toBeCloseTo(1, 10)
  expect(result.output[1] / -1e-150).toBeCloseTo(1, 10)
  await expect(page.getByRole('heading', { name: 'Float64 block output', exact: true })).toBeVisible()
  expect((await service.post('/api/profiles/read')).draft).toBeNull()
  await page.screenshot({ path: 'test-results/integration-processing-desktop.png', fullPage: true })
})

test('real processing import, slow warning, full FIR export and offline recovery stay usable', async ({ page, service }) => {
  await open(page, service)
  await page.getByRole('button', { name: 'Processing', exact: true }).click()
  const config = { name: 'Retained FIR', format: { sampleRateHz: 4294967295, channels: ['R', 'L'] }, effects: [{ name: 'Full impulse', enabled: true, effect: { kind: 'fir', coefficients: [1, ...Array(8192).fill(0), -0.125] } }] }
  await page.getByText('Import chain JSON', { exact: true }).click()
  await page.getByLabel('Or paste chain JSON', { exact: true }).fill(JSON.stringify(config))
  await page.getByRole('button', { name: 'Validate and import chain', exact: true }).click()
  await expect(page.getByLabel('Chain name', { exact: true })).toHaveValue('Retained FIR')
  await expect(page.getByText('Speed unknown. No current preview or timing.', { exact: false })).toBeVisible()
  await page.getByLabel('Block frames', { exact: true }).fill('1')
  await page.getByLabel('Timing repetitions', { exact: true }).fill('1')
  const response = page.waitForResponse(value => value.url().endsWith('/api/dsp/preview'))
  await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  const result = await (await response).json()
  expect(result.output).toEqual([0.25, 0.25])
  expect(result.admission.tail).toEqual({ kind: 'finite', frames: 8193 })
  if (result.timing.measured) {
    expect(result.timing.overBudget).toBe(true)
    await expect(page.getByRole('alert')).toContainText('Slower than the observed live block budget')
  }
  await expect(page.getByRole('button', { name: 'Render and measure', exact: true })).toBeEnabled()
  const download = page.waitForEvent('download')
  await page.getByRole('button', { name: 'Export chain', exact: true }).click()
  expect(JSON.parse(readFileSync((await (await download).path())!, 'utf8'))).toEqual(config)
  await page.setViewportSize({ width: 390, height: 844 })
  const dimensions = await page.evaluate(() => ({ width: innerWidth, body: document.documentElement.scrollWidth }))
  expect(dimensions.body).toBeLessThanOrEqual(dimensions.width)
  await page.screenshot({ path: 'test-results/integration-processing-mobile.png', fullPage: true })
  await service.stop()
  await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('Local service offline')
  await expect(page.getByLabel('Chain name', { exact: true })).toHaveValue('Retained FIR')
  await service.start()
  await page.getByRole('button', { name: 'Refresh', exact: true }).click()
  await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Float64 block output', exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Render and measure', exact: true })).toBeEnabled()
})
