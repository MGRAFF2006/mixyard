import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import { fileURLToPath } from 'node:url'
export default defineConfig({
  plugins: [vue()], base: './',
  ...(process.env.MIXYARD_MOBILE_CACHE_DIR ? { cacheDir: process.env.MIXYARD_MOBILE_CACHE_DIR } : {}),
  resolve: { alias: { vue: fileURLToPath(new URL('./node_modules/vue/dist/vue.runtime.esm-bundler.js', import.meta.url)) } },
  build: { target: ['es2022', 'chrome110', 'safari16'] },
})
