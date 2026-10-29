import assert from 'node:assert/strict'
import { once } from 'node:events'
import { createServer } from 'node:http'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { extname, join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { pinnedFetch } from './pinned-fetch.ts'

type Target = { id: string; displayName: string; deviceId: string; credential: string; url: string; certificate: string }

export async function verifyBrowser(root: string, directory: string, targets: Target[], revokeFirst: () => Promise<void>, readDraft: (id: string) => Promise<unknown>, stopFirst: () => Promise<void>, restartFirst: () => Promise<void>) {
  const require = createRequire(join(root, 'apps/web/package.json'))
  const { build } = await import(pathToFileURL(require.resolve('vite')).href)
  const { default: vue } = await import(pathToFileURL(require.resolve('@vitejs/plugin-vue')).href)
  const { chromium, expect } = require('@playwright/test')
  const output = join(directory, 'browser')
  await build({ configFile: false, root, cacheDir: join(directory, 'vite-cache'), plugins: [vue()], resolve: { alias: { vue: require.resolve('vue/dist/vue.runtime.esm-bundler.js') } }, logLevel: 'warn', build: { outDir: output, emptyOutDir: true, rollupOptions: { input: join(root, 'verification/device-control/browser.html') } } })
  const server = createServer((request, response) => {
    const path = resolve(output, '.' + new URL(request.url ?? '/', 'http://127.0.0.1').pathname)
    if (!path.startsWith(output + '/')) { response.writeHead(404).end(); return }
    try {
      const bytes = readFileSync(path)
      response.setHeader('Content-Type', ({ '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css' } as Record<string, string>)[extname(path)] ?? 'application/octet-stream')
      response.end(bytes)
    } catch { response.writeHead(404).end() }
  })
  const bound = once(server, 'listening')
  server.listen(0, '127.0.0.1'); await bound
  const address = server.address()
  assert.ok(address && typeof address !== 'string')
  let browser: Awaited<ReturnType<typeof chromium.launch>> | undefined
  const release = Promise.withResolvers<void>(), reached = Promise.withResolvers<void>()
  let holdNextWrite = false
  try {
    browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH } : {}) })
    const page = await browser.newPage({ viewport: { width: 390, height: 844 } })
    const transports = new Map(targets.map(target => [target.id, { target, fetch: pinnedFetch(target.url, target.certificate) }]))
    await page.exposeBinding('mixyardControlRequest', async (_source: unknown, id: string, path: string, init: { headers: Record<string, string>; body: string }) => {
      const transport = transports.get(id)
      assert.ok(transport, 'browser cannot invent a management endpoint')
      assert.ok(['/control/session', '/control/request'].includes(path))
      assert.ok(init.headers.authorization === `Bearer ${transport.target.credential}`, 'credentials must stay with their target')
      let response: Response
      try { response = await transport.fetch(path, { method: 'POST', headers: init.headers, body: init.body }) }
      catch (error) {
        if (error instanceof TypeError) return { kind: 'network-error' }
        throw error
      }
      const text = await response.text()
      if (holdNextWrite && id === targets[0]!.id && path === '/control/request' && JSON.parse(init.body).path === '/api/profiles/edit') {
        holdNextWrite = false; reached.resolve(); await release.promise
      }
      return { kind: 'response', status: response.status, text }
    })
    await page.addInitScript((values: Target[]) => { window.mixyardControlTargets = values }, targets.map(({ id, displayName, deviceId, credential }) => ({ id, displayName, deviceId, credential })))
    await page.goto(`http://127.0.0.1:${address.port}/verification/device-control/browser.html`)
    const wrongCredential = 'owned-wrong-bearer-sentinel'
    const mismatchDiagnostic = await page.evaluate(async ({ id, wrong }: { id: string; wrong: string }) => {
      try {
        await window.mixyardControlRequest(id, '/control/session', { headers: { authorization: `Bearer ${wrong}` }, body: '{}' })
        return ''
      } catch (error) { return String(error) }
    }, { id: targets[0]!.id, wrong: wrongCredential })
    assert.ok(mismatchDiagnostic.includes('credentials must stay with their target'), 'binding must reject a mismatched bearer')
    assert.ok(!mismatchDiagnostic.includes(wrongCredential) && targets.every(target => !mismatchDiagnostic.includes(target.credential)), 'binding diagnostics must not contain either bearer')
    const selector = page.getByLabel('Manage device', { exact: true })
    const gain = page.getByRole('slider', { name: 'mic to Call gain', exact: true })
    const save = page.getByRole('button', { name: 'Save draft', exact: true })
    await expect(selector).toHaveValue(targets[0]!.id)
    await expect(page.getByRole('button', { name: 'Load simulated example', exact: true })).toBeVisible()
    await page.getByRole('button', { name: 'Load simulated example', exact: true }).click()
    await gain.fill('0.37')
    await selector.selectOption(targets[1]!.id)
    await expect(gain).toHaveCount(0)
    await page.getByRole('button', { name: 'Load simulated example', exact: true }).click()
    await gain.fill('0.24')
    await expect(save).toBeEnabled(); await save.click()
    await expect(page.getByText('Draft matches stored copy', { exact: true })).toBeVisible()
    await selector.selectOption(targets[0]!.id)
    await expect(gain).toHaveValue('0.37')
    await expect(page.getByText('Unsaved draft edits', { exact: true })).toBeVisible()
    holdNextWrite = true
    await expect(save).toBeEnabled(); await save.click()
    await reached.promise
    await selector.selectOption(targets[1]!.id)
    await expect(gain).toHaveValue('0.24')
    release.resolve()
    await expect(page.getByText('Draft matches stored copy', { exact: true })).toBeVisible()
    const storedA = await readDraft(targets[0]!.id) as { sends: { source: string; mix: string; gain: number }[] }
    const storedB = await readDraft(targets[1]!.id) as typeof storedA
    assert.equal(storedA.sends.find(send => send.source === 'mic' && send.mix === 'call')?.gain, 0.37)
    assert.equal(storedB.sends.find(send => send.source === 'mic' && send.mix === 'call')?.gain, 0.24)
    await selector.selectOption(targets[0]!.id)
    await expect(gain).toHaveValue('0.37')
    await gain.fill('0.41')
    // This failure crosses the real Playwright binding, not the Node-only Api
    // path: errors must regain the browser realm's fetch semantics.
    await stopFirst()
    await page.getByRole('button', { name: 'Validate config', exact: true }).click()
    await expect(page.getByRole('alert')).toContainText(/offline or timed out/i)
    await expect(save).toBeDisabled()
    await expect(gain).toHaveValue('0.41')
    await restartFirst()
    await page.getByRole('button', { name: 'Refresh', exact: true }).click()
    await expect(save).toBeEnabled()
    await expect(gain).toHaveValue('0.41')
    await revokeFirst()
    await page.getByRole('button', { name: 'Refresh', exact: true }).click()
    await expect(page.getByRole('alert')).toContainText(/unauthorized|credential/i)
    await expect(gain).toHaveValue('0.41')
    await expect(save).toBeDisabled()
    await selector.selectOption(targets[1]!.id)
    await expect(gain).toHaveValue('0.24')
    const dimensions = await page.evaluate(() => ({ body: document.documentElement.scrollWidth, width: innerWidth }))
    assert.ok(dimensions.body <= dimensions.width, '390px management layout must not overflow')
  } finally {
    release.resolve()
    await browser?.close()
    server.closeAllConnections()
    await new Promise<void>((done, reject) => server.close(error => error ? reject(error) : done()))
  }
}
