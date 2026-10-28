import { spawn, spawnSync } from 'node:child_process'
import type { ChildProcess } from 'node:child_process'
import { once } from 'node:events'
import { chmodSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmdirSync, rmSync, unlinkSync } from 'node:fs'
import { createServer } from 'node:net'
import type { Socket } from 'node:net'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { createInterface } from 'node:readline'
import { fileURLToPath } from 'node:url'
import { test as base, expect } from '@playwright/test'
import type { Page } from '@playwright/test'
import { copyDsp, dspJson } from '../src/dsp-json.ts'
import { buildPreset, emptyPreset, emptyRequest } from '../src/situations.ts'
import type { SituationRequest } from '../src/situations.ts'

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

test('real channel operations and delay probe apply then verify exact alignment', async ({ page, service }) => {
  await open(page, service)
  await page.getByRole('button', { name: 'Processing', exact: true }).click()
  await page.getByRole('button', { name: 'Beginner', exact: true }).click()
  const shortcuts = page.getByRole('group', { name: 'Channel shortcuts', exact: true })
  await shortcuts.getByLabel('Reverse channel polarity', { exact: true }).check()
  await shortcuts.getByRole('button', { name: 'Add channel operation', exact: true }).click()
  await shortcuts.getByLabel('Operation', { exact: true }).selectOption('swap')
  await shortcuts.getByRole('button', { name: 'Add channel operation', exact: true }).click()
  await shortcuts.getByLabel('Operation', { exact: true }).selectOption('delay')
  await shortcuts.getByLabel('Delay (frames)', { exact: true }).fill('3')
  await shortcuts.getByRole('button', { name: 'Add channel operation', exact: true }).click()
  await page.getByLabel('Block frames', { exact: true }).fill('4')
  await page.getByLabel('Timing repetitions', { exact: true }).fill('1')
  await page.getByLabel('Test signal', { exact: true }).selectOption('samples')
  await page.getByLabel('Input samples (JSON array)', { exact: true }).fill('[1,10,0,0,0,0,0,0]')
  const output = page.waitForResponse(r => r.url().endsWith('/api/dsp/preview'))
  await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  expect((await (await output).json()).output).toEqual([0,-1,0,-0,0,-0,10,-0])
  await page.getByLabel('Channel 1 ID', { exact: true }).fill(' delayed / lane ')
  await page.getByLabel('Channel 2 ID', { exact: true }).focus()
  const delay = page.waitForResponse(r => r.url().endsWith('/api/dsp/delay'))
  await page.getByRole('button', { name: 'Measure signal delay', exact: true }).click()
  const measured = await (await delay).json()
  expect(measured.measurement.channels.map((c: { firstResponseFrame: number }) => c.firstResponseFrame)).toEqual([3,0])
  expect(measured.alignment.delays).toEqual([{ channel:' delayed / lane ',frames:0 },{ channel:'FR',frames:3 }])
  await expect(page.getByRole('region', { name: 'Measured signal delay', exact: true })).toBeVisible()
  await page.getByText('Measurement details and export', { exact:true }).click()
  const exportedFile = page.waitForEvent('download')
  await page.getByRole('button', { name:'Export delay measurement',exact:true }).click()
  const exported = JSON.parse(readFileSync((await (await exportedFile).path())!, 'utf8'))
  expect(exported.request.config.effects[0].channels).toEqual([' delayed / lane '])
  expect(exported.result.alignment).toEqual(measured.alignment)
  await page.getByRole('button', { name: 'Append proposed alignment delays', exact: true }).click()
  await expect(page.getByText('Signal delay unknown until measured', { exact: false })).toBeVisible()
  const again = page.waitForResponse(r => r.url().endsWith('/api/dsp/delay'))
  await page.getByRole('button', { name: 'Measure signal delay', exact: true }).click()
  const aligned = await (await again).json()
  expect(aligned.measurement.channels.map((c: { firstResponseFrame: number }) => c.firstResponseFrame)).toEqual([3,3])
  expect(aligned.alignment.delays.every((d: { frames: number }) => d.frames === 0)).toBe(true)
  await expect(page.getByRole('button', { name: 'Append proposed alignment delays', exact: true })).toBeDisabled()
  await page.screenshot({ path:'test-results/integration-channel-delay-desktop.png',fullPage:true })
  await page.setViewportSize({ width:390,height:844 })
  await expect(page.getByRole('region', { name: 'Measured signal delay', exact: true })).toBeVisible()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  await page.screenshot({ path:'test-results/integration-channel-delay-mobile.png',fullPage:true })
  await page.locator('.delay-meter').screenshot({ path:'test-results/integration-delay-meter-mobile.png' })
  expect((await service.post('/api/profiles/read')).draft).toBeNull()
})

test('applying milliseconds invalidates the previous waveform meters and processing timing', async ({ page, service }) => {
  await open(page, service)
  await page.getByRole('button', { name:'Processing',exact:true }).click()
  await page.getByLabel('New effect', { exact:true }).selectOption('delay')
  await page.getByRole('button', { name:'Add effect',exact:true }).click()
  await page.getByLabel('Timing repetitions', { exact:true }).fill('1')
  await page.getByText('Set delay in milliseconds', { exact:true }).click()
  await page.getByLabel('Requested delay (ms)', { exact:true }).fill('1')
  await page.getByRole('button', { name:'Render and measure',exact:true }).click()
  await expect(page.getByRole('heading', { name:'Float64 block output',exact:true })).toBeVisible()
  await page.getByRole('button', { name:'Apply milliseconds',exact:true }).click()
  await expect(page.getByLabel('Delay (frames)', { exact:true })).toHaveValue('48')
  await expect(page.getByRole('heading', { name:'Float64 block output',exact:true })).toHaveCount(0)
  await expect(page.getByRole('heading', { name:'Measured processing time',exact:true })).toHaveCount(0)
})

test('valid extreme Float64 gains remain renderable and exportable through the derived dB control', async ({ page, service }) => {
  await open(page, service)
  await page.getByRole('button', { name:'Processing',exact:true }).click()
  await page.getByText('Import chain JSON', { exact:true }).click()
  for (const gain of [1e-310, 1e308, Number.MIN_VALUE, Number.MAX_VALUE]) {
    const config = { name:'Extreme linear gain',format:{sampleRateHz:48000,channels:['one','two']},effects:[{ name:'Exact gain',enabled:true,effect:{kind:'gain',gain,invertPolarity:false} }] }
    await page.getByLabel('Or paste chain JSON', { exact:true }).fill(JSON.stringify(config))
    await page.getByRole('button', { name:'Validate and import chain',exact:true }).click()
    await expect(page.getByText('Chain imported; core validation passed.', { exact:false })).toBeVisible()
    await page.getByLabel('Block frames', { exact:true }).fill('1')
    await page.getByLabel('Timing repetitions', { exact:true }).fill('1')
    await page.getByLabel('Test signal', { exact:true }).selectOption('samples')
    await page.getByLabel('Input samples (JSON array)', { exact:true }).fill('[1,1]')
    const response = page.waitForResponse(r => r.url().endsWith('/api/dsp/preview'))
    await page.getByRole('button', { name:'Render and measure',exact:true }).click()
    await expect(page.getByRole('heading', { name:'Float64 block output',exact:true })).toBeVisible({timeout:5000})
    expect((await (await response).json()).output).toEqual([gain,gain])
    const download = page.waitForEvent('download')
    await page.getByRole('button', { name:'Export chain',exact:true }).click()
    const exported = JSON.parse(readFileSync((await (await download).path())!, 'utf8'))
    expect(exported.effects[0].effect.gain).toBe(gain)
  }
  await page.getByLabel('Level (dB)', { exact:true }).fill('6165.094311198335')
  await expect(page.getByRole('alert')).toContainText('previous gain is kept')
  expect(Number(await page.getByLabel('Linear gain', { exact:true }).inputValue())).toBe(Number.MAX_VALUE)
  await expect(page.getByRole('heading', { name:'Float64 block output',exact:true })).toHaveCount(0)
})


test('real situation evaluator imports and explains explicit fixture decisions without writing profiles', async ({ page, service }) => {
  await open(page, service)
  await page.getByRole('button', { name: 'Situations', exact: true }).click()
  await page.getByText('Expert JSON / import full configuration', { exact: true }).click()
  const input = page.getByLabel('Full situation configuration JSON', { exact: true })
  for (const [name, terminal] of [['headphone-priority', 'headphones'], ['interface-monitor', 'suppress'], ['application-schedule', 'call']]) {
    const request = readFileSync(join(root, `examples/automation/${name}.json`), 'utf8')
    await input.fill(request)
    await page.getByRole('button', { name: 'Validate and import JSON', exact: true }).click()
    await expect(page.getByText('Full configuration imported after core validation', { exact: false })).toBeVisible()
    await page.getByRole('button', { name: 'Simulate configuration', exact: true }).click()
    await expect(page.locator('.situation-result .message.success')).toContainText(`Terminal: ${terminal}. Audio applied: no.`)
  }
  expect((await service.post('/api/profiles/read')).storeRevision).toBe(0)
  await page.getByRole('button', { name: 'Read live context', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Copy read-only context into simulation', exact: true })).toBeVisible()
  await expect(page.locator('.situation-providers')).toContainText('unimplemented: unsupported')
  await page.screenshot({ path: 'test-results/integration-situations-desktop.png', fullPage: true })
})

test('scoped context fixture keeps unmapped negative application rules unknown through actual evaluator and export', async ({ page, service }) => {
  await open(page, service)
  await page.getByRole('button', { name: 'Situations', exact: true }).click()
  const request = buildPreset({ ...emptyPreset(), kind: 'applications', apps: ['UNMAPPED'], profile: 'active', normalProfile: 'normal' }, emptyRequest())
  const node = request.graph.nodes[0]!
  if (node.kind !== 'decision') throw Error('fixture')
  node.condition = { kind: 'not', condition: node.condition }
  await page.getByText('Expert JSON / import full configuration', { exact: true }).click()
  await page.getByLabel('Full situation configuration JSON', { exact: true }).fill(dspJson(request))
  await page.getByRole('button', { name: 'Validate and import JSON', exact: true }).click()
  await expect(page.getByText('Full configuration imported after core validation', { exact: false })).toBeVisible()
  // Only this observation is a fixture. Evaluation uses the production Rust API.
  await page.route('**/api/automation/context', route => route.fulfill({ json: { mode: 'read-only-live-context', catalog: request.catalog, nowMs: 0, snapshot: { schemaVersion: 1, revision: 1, epoch: 'scoped-fixture', facts: [{ key: { name: 'application.running' }, status: 'known', value: { type: 'text-set', value: [] }, provider: 'scoped-test-fixture', evidence: 'fixture', observedAtMs: 0, validForMs: 5000, completeness: 'complete' }] }, applicationScope: ['MAPPED'], providers: [{ provider: 'scoped-test-fixture', status: 'complete', reason: 'Complete only over MAPPED' }] } }))
  await page.getByRole('button', { name: 'Read live context', exact: true }).click()
  await page.getByRole('button', { name: 'Copy read-only context into simulation', exact: true }).click()
  await page.getByRole('button', { name: 'Simulate configuration', exact: true }).click()
  await expect(page.locator('.situation-result .message.success')).toContainText('No change. Terminal: safe.')
  await expect(page.locator('.situation-trace')).toContainText('unknown')
  const downloaded = page.waitForEvent('download')
  await page.getByRole('button', { name: 'Export full configuration', exact: true }).first().click()
  const exported = JSON.parse(readFileSync((await (await downloaded).path())!, 'utf8'))
  expect(exported.snapshot.facts[0].completeness).toBe('partial')
  await page.setViewportSize({ width: 390, height: 844 })
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
  await page.screenshot({ path: 'test-results/integration-situations-mobile.png', fullPage: true })
  expect((await service.post('/api/profiles/read')).storeRevision).toBe(0)
})


test('numeric validity follows replaced operands and surviving condition, fact, membership and reduction rows', async ({ page, service }) => {
  await open(page, service)
  await page.getByRole('button', { name: 'Situations', exact: true }).click()
  await page.getByText('Expert JSON / import full configuration', { exact: true }).click()
  const numericCondition = (value: number) => ({ kind: 'fact' as const, key: { name: 'civil.minute-of-day' }, predicate: { op: 'lt', value } })
  async function load(request: SituationRequest) {
    await page.getByLabel('Full situation configuration JSON', { exact: true }).fill(dspJson(request))
    await page.getByRole('button', { name: 'Validate and import JSON', exact: true }).click()
    await expect(page.getByText('Full configuration imported after core validation', { exact: false })).toBeVisible()
  }
  async function simulate() {
    await page.getByRole('button', { name: 'Simulate configuration', exact: true }).click()
    await expect(page.locator('.situation-result .message.success')).toContainText('Terminal: safe.')
  }
  const request = emptyRequest()
  request.graph.root = 'when'
  request.graph.nodes.unshift({ id: 'when', kind: 'decision', condition: numericCondition(10), onTrue: 'safe', onFalse: 'safe', onUnknown: 'safe' })
  await load(request)
  const number = page.getByLabel('Numeric comparison value', { exact: true })
  await number.fill('')
  await page.getByLabel('Graph name', { exact: true }).fill('Keep invalid numeric draft')
  await expect(number).toHaveValue('')
  await expect.poll(() => number.evaluate(e => (e as HTMLInputElement).validity.valid)).toBe(false)
  await page.getByLabel('Comparison', { exact: true }).selectOption('le')
  await expect(number).toHaveValue('0')
  await expect.poll(() => number.evaluate(e => (e as HTMLInputElement).validity.valid)).toBe(true)
  await simulate()
  const node = request.graph.nodes[0]!
  if (node.kind !== 'decision') throw Error('fixture')
  node.condition = { kind: 'all', conditions: [numericCondition(20), numericCondition(20)] }
  await load(request)
  await number.first().fill('')
  await page.getByRole('button', { name: 'Remove Condition.1', exact: true }).click()
  await expect(number).toHaveValue('20')
  await expect.poll(() => number.evaluate(e => (e as HTMLInputElement).validity.valid)).toBe(true)
  await simulate()
  request.snapshot.facts = ['civil.minute-of-day', 'civil.weekday'].map(name => ({ key: { name }, status: 'unknown', provider: 'fixture', evidence: 'fixture', observedAtMs: 0, validForMs: 5000, completeness: 'partial' }))
  await load(request)
  await page.getByText('Simulation facts (2)', { exact: true }).click()
  await page.getByLabel('Valid for (ms)', { exact: true }).first().fill('-1')
  await page.getByRole('button', { name: 'Remove fact 1', exact: true }).click()
  const duration = page.getByLabel('Valid for (ms)', { exact: true })
  await expect(duration).toHaveValue('5000')
  await expect.poll(() => duration.evaluate(e => (e as HTMLInputElement).validity.valid)).toBe(true)
  await simulate()
  request.snapshot.facts = []
  node.condition = { kind: 'fact', key: { name: 'civil.minute-of-day' }, predicate: { op: 'in', values: [{ type: 'number', value: 10 }, { type: 'number', value: 20 }] } }
  await load(request)
  await page.getByLabel('Membership value 1', { exact: true }).fill('')
  await page.getByRole('button', { name: 'Remove membership value 1', exact: true }).click()
  const member = page.getByLabel('Membership value 1', { exact: true })
  await expect(member).toHaveValue('20')
  await expect.poll(() => member.evaluate(e => (e as HTMLInputElement).validity.valid)).toBe(true)
  await simulate()
  request.graph.nodes.push({ id: 'reductions', kind: 'terminal', action: { kind: 'select', overrides: [{ scope: 'output', output: 'one', gain: 0.1 }, { scope: 'output', output: 'two', gain: 0.2 }] } })
  await load(request)
  await page.getByRole('button', { name: 'Edit reductions', exact: true }).click()
  await page.getByLabel('Attenuation gain (optional, 0–1)', { exact: true }).first().fill('2')
  await page.getByRole('button', { name: 'Remove reduction 1', exact: true }).click()
  const gain = page.getByLabel('Attenuation gain (optional, 0–1)', { exact: true })
  await expect(gain).toHaveValue('0.2')
  await expect.poll(() => gain.evaluate(e => (e as HTMLInputElement).validity.valid)).toBe(true)
  await simulate()
})


test('deleting another row preserves the surviving invalid draft in every numeric list', async ({ page, service }) => {
  const api = { requests: [] as { path: string }[] }
  page.on('request', request => { api.requests.push({ path: new URL(request.url()).pathname }) })
  await open(page, service)
  await page.getByRole('button', { name: 'Situations', exact: true }).click()
  await page.getByText('Expert JSON / import full configuration', { exact: true }).click()
  for (const kind of ['conditions', 'membership', 'facts', 'reductions']) {
    for (const removed of [0, 1]) {
      const request = emptyRequest(), numeric = { kind: 'fact' as const, key: { name: 'civil.minute-of-day' }, predicate: { op: 'lt', value: 20 } }
      request.graph.root = 'when'
      request.graph.nodes.unshift({ id: 'when', kind: 'decision', condition: numeric, onTrue: 'safe', onFalse: 'safe', onUnknown: 'safe' })
      const node = request.graph.nodes[0]!
      if (node.kind !== 'decision') throw Error('fixture')
      if (kind === 'conditions') node.condition = { kind: 'all', conditions: [copyDsp(numeric), copyDsp(numeric)] }
      if (kind === 'membership') node.condition = { kind: 'fact', key: numeric.key, predicate: { op: 'in', values: [{ type: 'number', value: 20 }, { type: 'number', value: 30 }] } }
      if (kind === 'facts') request.snapshot.facts = ['civil.minute-of-day', 'civil.weekday'].map(name => ({ key: { name }, status: 'unknown', provider: 'fixture', evidence: 'fixture', observedAtMs: 0, validForMs: 5000, completeness: 'partial' }))
      if (kind === 'reductions') request.graph.nodes.push({ id: 'reductions', kind: 'terminal', action: { kind: 'select', overrides: [{ scope: 'output', output: 'one', gain: 0.2 }, { scope: 'output', output: 'two', gain: 0.2 }] } })
      await page.getByLabel('Full situation configuration JSON', { exact: true }).fill(dspJson(request))
      await page.getByRole('button', { name: 'Validate and import JSON', exact: true }).click()
      await expect(page.getByText('Full configuration imported after core validation', { exact: false })).toBeVisible()
      if (kind === 'facts') await page.getByText('Simulation facts (2)', { exact: true }).click()
      if (kind === 'reductions') await page.getByRole('button', { name: 'Edit reductions', exact: true }).click()
      const controls = kind === 'conditions' ? page.getByLabel('Numeric comparison value', { exact: true })
        : kind === 'membership' ? page.locator('.situation-node-editor input[type="number"]')
          : kind === 'facts' ? page.getByLabel('Valid for (ms)', { exact: true })
            : page.getByLabel('Attenuation gain (optional, 0–1)', { exact: true })
      const invalid = kind === 'facts' ? '-1' : kind === 'reductions' ? '2' : ''
      await controls.nth(1 - removed).fill(invalid)
      const button = kind === 'conditions' ? `Remove Condition.${removed + 1}` : kind === 'membership' ? `Remove membership value ${removed + 1}` : kind === 'facts' ? `Remove fact ${removed + 1}` : `Remove reduction ${removed + 1}`
      await page.getByRole('button', { name: button, exact: true }).click()
      await expect(controls).toHaveCount(1)
      await expect(controls).toHaveValue(invalid)
      await expect.poll(() => controls.evaluate(e => (e as HTMLInputElement).validity.valid)).toBe(false)
      const before = api.requests.filter(r => r.path.endsWith('/evaluate')).length
      await page.getByRole('button', { name: 'Simulate configuration', exact: true }).click()
      expect(api.requests.filter(r => r.path.endsWith('/evaluate')).length).toBe(before)
      await controls.fill(kind === 'facts' ? '5000' : kind === 'reductions' ? '0.2' : '20')
      await page.getByRole('button', { name: 'Simulate configuration', exact: true }).click()
      await expect(page.locator('.situation-result .message.success')).toContainText('Terminal: safe.')
    }
  }
})


// Explicit simulated inventory over actual protected private IPC. The evaluator,
// controller, review, HTTP service and optimistic disk writer remain production.
async function privateInventory(service: Service) {
  const directory = join(process.platform === 'darwin' ? '/private/tmp' : '/tmp', `mixyard-${process.getuid!()}-${service.instance}`)
  mkdirSync(directory, { mode: 0o700 })
  const identity = lstatSync(directory), path = join(directory, 'runtime.sock'), sockets = new Set<Socket>(), commands: string[] = []
  let paused = false
  const pendingReplies: (() => void)[] = []
  const server = createServer(socket => {
    sockets.add(socket); socket.on('close', () => sockets.delete(socket)); socket.on('error', () => {})
    socket.setTimeout(5000, () => socket.destroy())
    let data = Buffer.alloc(0), replied = false
    socket.on('data', chunk => {
      if (replied) return
      data = Buffer.concat([data, chunk])
      if (data.length > 65536) { socket.destroy(); return }
      const end = data.indexOf(10)
      if (end < 0) return
      replied = true
      try {
        const request = JSON.parse(data.subarray(0, end).toString('utf8'))
        commands.push(request.command)
        const envelope = request.command === 'inventory'
          ? { ipcVersion: 1, requestId: request.requestId, result: { schemaVersion: 2, revision: 1, evidence: 'simulated', platform: 'linux', sources: [], outputs: [], mediaSessions: [] } }
          : { ipcVersion: 1, requestId: request.requestId, error: { code: 'fixture-inventory-only', message: 'Explicit simulated inventory fixture' } }
        const reply = () => { if (!socket.destroyed) socket.end(JSON.stringify(envelope) + '\n') }
        if (paused) pendingReplies.push(reply); else reply()
      } catch { socket.destroy() }
    })
  })
  await new Promise<void>((resolve, reject) => { server.once('error', reject); server.listen(path, () => { server.removeListener('error', reject); resolve() }) })
  chmodSync(path, 0o600)
  const socketIdentity = lstatSync(path)
  return {
    commands,
    pause() { paused = true },
    resume() { paused = false; for (const reply of pendingReplies.splice(0)) reply() },
    async stop() {
      for (const socket of sockets) socket.destroy()
      await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()))
      // Only remove this fixture's endpoint, never a replacement or shared path.
      try { if (lstatSync(path).ino === socketIdentity.ino) unlinkSync(path) } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error }
      if (lstatSync(directory).ino === identity.ino) rmdirSync(directory)
    },
  }
}
async function seedAutomatic(service: Service) {
  const config = (gain: number, revision: number) => ({ schemaVersion: 2, revision, sources: [], mixes: [{ id: 'desk', name: 'Desk', channels: 2, master: { mute: false, gain } }], sends: [], outputs: [], mediaControls: [] })
  const profile = await service.post('/api/profiles/edit', { expectedRevision: 0, edit: { kind: 'save', id: null, name: 'Quiet', config: config(0.25, 7) } })
  await service.post('/api/profiles/edit', { expectedRevision: 1, edit: { kind: 'draft', config: config(1, 1) } })
  const graph = { schemaVersion: 1, id: 'browser-policy', name: 'Browser automatic policy', revision: 1, root: 'rule', nodes: [
    { kind: 'decision', id: 'rule', condition: { kind: 'fact', key: { name: 'manual.flag', subject: 'desk' }, predicate: { op: 'eq', value: { type: 'bool', value: true } } }, onTrue: 'selected', onFalse: 'hold', onUnknown: 'hold' },
    { kind: 'terminal', id: 'selected', action: { kind: 'select', profileId: profile.profiles[0].id, overrides: [] } },
    { kind: 'terminal', id: 'hold', action: { kind: 'no-change' } },
  ] }
  await service.post('/api/automation/settings/write', { expectedSettingsRevision: 0, settings: { graph, catalog: emptyRequest().catalog, declarations: [{ key: { name: 'manual.flag', subject: 'desk' }, value: true }], transitionPolicy: { settleMs: 0, minimumHoldMs: 0, cooldownMs: 0 }, observer: { applications: {}, hostLabel: null, timezone: 'Europe/Berlin' } } })
}
async function openAutomatic(page: Page, service: Service) {
  await open(page, service)
  await page.getByRole('button', { name: 'Situations', exact: true }).click()
  const panel = page.getByRole('region', { name: 'Automatic selection', exact: true })
  await panel.getByRole('button', { name: 'Read saved settings', exact: true }).click()
  await panel.getByRole('button', { name: 'Load saved graph into builder', exact: true }).click()
  await expect(page.getByLabel('Graph name', { exact: true })).toHaveValue('Browser automatic policy')
  await panel.getByRole('button', { name: 'Clear manual pause', exact: true }).click()
  return panel
}

test('actual automatic controls review and commit once through real HTTP private IPC and disk; restart stays disarmed', async ({ page, service }) => {
  test.skip(process.platform !== 'linux', 'Owned Unix IPC inventory fixture is qualified on Linux only')
  await seedAutomatic(service)
  const panel = await openAutomatic(page, service), runtime = await privateInventory(service)
  try {
    await panel.getByRole('button', { name: 'Review automatic selection', exact: true }).click()
    await expect(panel.getByText('Review covers 1 reachable Select actions.', { exact: false })).toBeVisible()
    const arm = panel.getByRole('button', { name: 'Arm reviewed automatic selection', exact: true })
    await expect(arm).toBeDisabled()
    await panel.getByLabel('I have inspected every candidate and sensitive path.', { exact: true }).check()
    await expect(arm).toBeEnabled()
    await page.screenshot({ path: 'test-results/integration-automatic-review-desktop.png', fullPage: true })
    await arm.click()
    await expect.poll(async () => (await service.post('/api/profiles/read')).storeRevision).toBe(3)
    const stored = await service.post('/api/profiles/read')
    expect(stored.draft.mixes[0].master.gain).toBe(0.25)
    await panel.getByRole('button', { name: 'Refresh selection status', exact: true }).click()
    await expect(panel.getByRole('heading', { name: 'Armed', exact: true })).toBeVisible()
    await panel.getByRole('button', { name: 'Pause until cleared', exact: true }).click()
    await expect(panel.getByRole('heading', { name: 'Manually paused / armed authority retained', exact: true })).toBeVisible()
    await panel.getByRole('button', { name: 'Clear manual pause', exact: true }).click()
    await panel.getByRole('button', { name: 'Disarm automatic selection', exact: true }).click()
    await expect(panel.getByRole('heading', { name: 'Disarmed', exact: true })).toBeVisible()
    expect((await service.post('/api/profiles/read')).storeRevision).toBe(3)
    expect(runtime.commands.filter(c => c === 'inventory').length).toBeGreaterThan(1)
    expect(runtime.commands.some(c => ['start', 'shutdown', 'apply', 'stop'].includes(c))).toBe(false)
  } finally { await runtime.stop() }
  // Keep the same browser, process-local manual baseline and unsaved settings.
  await panel.getByLabel('Settle time (ms)', { exact: true }).fill('777')
  await service.stop(); await service.start()
  await page.getByRole('button', { name: 'Refresh', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Refresh', exact: true })).toBeEnabled()
  await panel.getByRole('button', { name: 'Read saved settings', exact: true }).click()
  await expect(panel.getByRole('heading', { name: 'Disarmed', exact: true })).toBeVisible()
  await expect(panel.getByLabel('Settle time (ms)', { exact: true })).toHaveValue('777')
  await expect(panel.getByRole('button', { name: 'Clear manual pause', exact: true })).toBeEnabled()
  expect((await service.post('/api/profiles/read')).storeRevision).toBe(3)
})

test('actual missing inventory blocks arming without a stored write and narrow controls remain usable', async ({ page, service }) => {
  await seedAutomatic(service)
  const panel = await openAutomatic(page, service)
  await panel.getByRole('button', { name: 'Review automatic selection', exact: true }).click()
  await expect(panel.getByText('inventory-unavailable', { exact: false }).first()).toBeVisible()
  await expect(panel.getByRole('button', { name: 'Arm reviewed automatic selection', exact: true })).toHaveCount(0)
  expect((await service.post('/api/profiles/read')).storeRevision).toBe(2)
  await page.setViewportSize({ width: 390, height: 844 })
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
  await page.screenshot({ path: 'test-results/integration-automatic-blocked-mobile.png', fullPage: true })
  await panel.getByRole('button', { name: 'Disarm automatic selection', exact: true }).click()
  expect((await service.post('/api/automation/status')).armed).toBe(false)
})


test('actual Disarm cancels slow pending Arm without a stored write or late authority restoration', async ({ page, service }) => {
  test.skip(process.platform !== 'linux', 'Owned Unix IPC inventory fixture is qualified on Linux only')
  await seedAutomatic(service)
  const panel = await openAutomatic(page, service), runtime = await privateInventory(service)
  try {
    await panel.getByRole('button', { name: 'Review automatic selection', exact: true }).click()
    await panel.getByLabel('I have inspected every candidate and sensitive path.', { exact: true }).check()
    const count = runtime.commands.length
    runtime.pause()
    await panel.getByRole('button', { name: 'Arm reviewed automatic selection', exact: true }).click()
    await expect.poll(() => runtime.commands.length).toBeGreaterThan(count)
    const disarm = panel.getByRole('button', { name: 'Disarm automatic selection', exact: true })
    // Check immediately while inventory is withheld; auto-waiting could hide
    // the defect by waiting for the Arm request to time out first.
    expect(await disarm.isEnabled()).toBe(true); await disarm.click()
    await expect(panel.getByRole('status').filter({ hasText: 'Automatic selection disarmed' })).toBeVisible()
    runtime.resume()
    await panel.getByRole('button', { name: 'Refresh selection status', exact: true }).click()
    await expect(panel.getByRole('heading', { name: 'Disarmed', exact: true })).toBeVisible()
    expect((await service.post('/api/profiles/read')).storeRevision).toBe(2)
    expect((await service.post('/api/automation/status')).armed).toBe(false)
  } finally { await runtime.stop() }
})
