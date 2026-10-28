import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

// API calls intentionally stay same-origin. Use the Rust server for integration.
export default defineConfig({
  plugins: [vue()],
  server: { port: 5174, strictPort: true },
  build: { target: ['es2022', 'chrome110', 'safari16'] },
})
