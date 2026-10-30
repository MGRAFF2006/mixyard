import { createRequire } from 'node:module'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
const require = createRequire(resolve('apps/web/package.json'))
const { build } = await import(pathToFileURL(require.resolve('vite')).href)
const { default: vue } = await import(pathToFileURL(require.resolve('@vitejs/plugin-vue')).href)
const destination = process.argv[2]
if (!destination || !process.env.MIXYARD_CONFIRMATION_CACHE) throw Error('Set an owned output and MIXYARD_CONFIRMATION_CACHE.')
await build({ configFile: false, root: resolve('verification/mobile-confirmations/fixture'), base: './', cacheDir: process.env.MIXYARD_CONFIRMATION_CACHE,
  plugins: [vue()], resolve: { alias: { vue: require.resolve('vue/dist/vue.runtime.esm-bundler.js') } },
  build: { outDir: resolve(destination), emptyOutDir: true }, logLevel: 'warn' })
