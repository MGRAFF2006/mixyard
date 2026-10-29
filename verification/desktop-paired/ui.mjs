// Render the production desktop Vue entry. Only the native invoke host and vault are API doubles;
// pair/session/request operations run in the actual production Rust library against real TLS.
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createRequire } from 'node:module'
import { createInterface } from 'node:readline'
import { createServer } from 'node:net'
import { once } from 'node:events'
import { readFile, writeFile } from 'node:fs/promises'
import { resolve } from 'node:path'

let input = ''
for await (const chunk of process.stdin) input += chunk
const setup = JSON.parse(input)
const require = createRequire(resolve('apps/web/package.json'))
const { chromium } = require('@playwright/test')
const native = spawn(setup.harness, [], { stdio: ['pipe', 'pipe', 'pipe'] })
const pending = []
createInterface({ input: native.stdout }).on('line', line => {
  const callback = pending.shift()
  if (callback) callback(JSON.parse(line))
})
const calls = []
async function rpc(op, args = {}) {
  const response = new Promise(resolve => pending.push(resolve))
  native.stdin.write(JSON.stringify({ op, ...args }) + '\n')
  const result = await response
  if ('error' in result) throw Error(result.error)
  return result.ok
}
async function invoke(command, args = {}) {
  calls.push(command)
  switch (command) {
    case 'shell_status': return { schemaVersion: 1, connected: false, runtime: { phase: 'disconnected', instance: null, event: null, error: null }, trayCreated: false, platform: 'linux', compact: false }
    case 'take_ui_action': return null
    case 'update_tray_context': return null
    case 'paired_list': return rpc('list')
    case 'paired_pair': return rpc('pair', { endpoint: args.endpoint, name: args.displayName, invitation: JSON.parse(args.invitation) })
    case 'paired_open': return rpc('open', { id: args.targetId, credentialId: args.credentialId })
    case 'paired_request': return rpc('request', { id: args.targetId, generation: args.generation, raw: args.raw })
    case 'paired_disconnect': return rpc('disconnect', { id: args.targetId, generation: args.expectedGeneration })
    case 'paired_forget': return rpc('forget', { id: args.targetId })
    default: throw Error('Unavailable isolated UI host command')
  }
}
const reserve = createServer().listen(0, '127.0.0.1')
await once(reserve, 'listening')
const port = reserve.address().port
await new Promise(resolve => reserve.close(resolve))
const vite = spawn(process.execPath, [resolve('apps/desktop/node_modules/vite/bin/vite.js'), '--host', '127.0.0.1', '--port', String(port), '--strictPort'], {
  cwd: resolve('apps/desktop'), env: { ...process.env, MIXYARD_VITE_CACHE: resolve(setup.cache) }, stdio: ['ignore', 'pipe', 'pipe'],
})
let browser, stage = 'start owned Vite'
try {
  const started = Date.now()
  while (true) {
    try { if ((await fetch(`http://127.0.0.1:${port}`)).ok) break } catch { /* Owned server startup only. */ }
    if (Date.now() - started > 20_000) throw Error('Owned UI server startup deadline')
    await new Promise(resolve => setTimeout(resolve, 100))
  }
  browser = await chromium.launch({ headless: true })
  const context = await browser.newContext({ viewport: { width: 360, height: 800 }, permissions: [] })
  const page = await context.newPage()
  page.setDefaultTimeout(15_000)
  await page.exposeBinding('ownedNativeInvoke', (_source, command, args) => invoke(command, args))
  await page.addInitScript(() => {
    window.__TAURI_INTERNALS__ = { invoke: (command, args) => window.ownedNativeInvoke(command, args) }
  })
  await page.goto(`http://127.0.0.1:${port}`)
  stage = 'render empty saved-device flow'
  await page.getByText('Pair and manage devices · native TLS and secure vault', { exact: true }).click()
  await page.getByText('No saved paired devices. The local loopback CSRF target remains separate.').waitFor()
  stage = 'explicit OOB onboarding'
  await page.getByLabel('Device name', { exact: true }).fill('Owned TLS device')
  await page.getByLabel('Exact private endpoint', { exact: true }).fill(setup.endpoint)
  await page.getByLabel('Out-of-band invitation JSON', { exact: true }).fill(JSON.stringify(setup.invitation))
  await page.getByLabel('I verified this device ID and certificate pin through the owning device’s trusted channel.').check()
  await page.getByRole('button', { name: 'Pair and save in secure vault', exact: true }).click()
  const selector = page.getByLabel('Manage device', { exact: true })
  await selector.locator(`option[value^="paired:${setup.invitation.deviceId}:"]`).waitFor({ state: 'attached' })
  assert.equal(await page.getByLabel('Out-of-band invitation JSON', { exact: true }).inputValue(), '')
  stage = 'select paired host through production shared controls'
  const paired = (await rpc('list'))[0]
  const targetKey = `paired:${paired.deviceId}:${paired.credentialId}`
  await selector.selectOption(targetKey)
  await page.getByText('Connected', { exact: true }).waitFor()
  await page.getByRole('button', { name: 'Load simulated example', exact: true }).click()
  await page.getByRole('button', { name: 'Save draft', exact: true }).click()
  await page.getByText('Draft matches stored copy', { exact: true }).waitFor()
  const open = await rpc('open', { id: setup.invitation.deviceId, credentialId: paired.credentialId })
  const session = JSON.parse(open.body)
  const read = await rpc('request', { id: setup.invitation.deviceId, generation: open.generation,
    raw: JSON.stringify({ deviceId: session.deviceId, authorityId: session.authorityId, revision: session.revision, path: '/api/profiles/read', body: {} }) })
  assert.equal(JSON.parse(read.body).body.storeRevision, 1)
  stage = 'dirty target blocks removal and list rebuilding'
  await page.getByLabel('New mix name', { exact: true }).fill('Retained unsaved host text')
  await page.getByRole('button', { name: 'Refresh saved target list', exact: true }).click()
  assert.equal(await page.getByLabel('New mix name', { exact: true }).inputValue(), 'Retained unsaved host text')
  page.once('dialog', dialog => dialog.dismiss())
  await page.getByRole('button', { name: 'Remove saved credential', exact: true }).click()
  assert.equal((await rpc('list')).length, 1)
  stage = 'target switching retains dirty cached host workspace'
  await selector.selectOption('local')
  await selector.selectOption(targetKey)
  assert.equal(await page.getByLabel('New mix name', { exact: true }).inputValue(), 'Retained unsaved host text')
  await page.getByLabel('New mix name', { exact: true }).fill('')
  stage = 'small-screen onboarding and target identity remain readable'
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1), true)
  await page.screenshot({ path: setup.screenshot, fullPage: true })
  assert.ok(calls.includes('paired_pair') && calls.includes('paired_open') && calls.includes('paired_request'))
  process.stdout.write(JSON.stringify({ status: 'passed', viewport: '360x800', checks: ['rendered OOB invitation cleared after production native pairing', 'shared target selector to actual TLS session and persisted draft', 'saved target reconciliation preserves dirty cached host; removal guard and switching retain work', 'small-screen document has no horizontal overflow'], boundary: 'Chromium + native invoke/vault API doubles; production Rust TLS/service; no actual Tauri webview or OS vault success claim' }) + '\n')
} catch (error) {
  // Never print a Playwright call log: a fill failure may include the one-use invitation.
  process.stderr.write(`Owned rendered desktop qualification failed at: ${stage}; ${error?.name ?? 'Error'}\n`)
  process.exitCode = 1
} finally {
  await browser?.close()
  native.stdin.end()
  await once(native, 'exit')
  vite.kill('SIGTERM')
  await once(vite, 'exit')
}
