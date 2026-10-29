import { cp, mkdir, readFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
const mobile = new URL('../', import.meta.url)
const app = await readFile(new URL('../../web/src/App.vue', import.meta.url), 'utf8')
// Silently using ConsoleSession(new Api()) would bypass the native identity boundary.
if (!/props\.api|defineProps<[^>]*api[?]?:\s*Api/s.test(app)) throw new Error('Shared App.vue API injection seam missing. Integrate the shared controller PR before packaging mobile.')
const source = new URL('controller/dist/', mobile)
await readFile(new URL('index.html', source))
for (const target of ['android/app/src/main/assets/controller/', 'ios/App/controller/']) {
  const destination = new URL(target, mobile)
  await mkdir(destination, { recursive: true })
  await cp(source, destination, { recursive: true, force: true })
  process.stdout.write(`Staged controller: ${fileURLToPath(destination)}\n`)
}
