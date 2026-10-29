import { defineConfig } from '@playwright/test'
// Focused portable UI qualification; no Rust/native service or audio is launched.
export default defineConfig({
  testDir: './tests', testMatch: ['**/network-codecs.spec.ts', '**/devices.spec.ts'], workers: 1,
  outputDir: process.env.MIXYARD_CODEC_TEST_OUTPUT ?? 'test-results/codecs',
  use: {
    baseURL: 'http://127.0.0.1:5187', headless: true, screenshot: 'off', trace: 'off',
    launchOptions: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH } : {},
  },
  webServer: { command: 'node node_modules/vite/bin/vite.js --host 127.0.0.1 --port 5187', url: 'http://127.0.0.1:5187', reuseExistingServer: false },
})
