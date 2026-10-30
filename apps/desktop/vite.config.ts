import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  resolve: { dedupe: ['vue'] },
  cacheDir: process.env.MIXYARD_VITE_CACHE ?? 'node_modules/.vite',
  server: { port: 1420, strictPort: true },
  envPrefix: ['VITE_'],
  build: { target: ['es2022', 'chrome110', 'safari16'] },
})
