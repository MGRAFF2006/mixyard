import { cp, mkdir, readFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { createHash } from 'node:crypto'
import { Script } from 'node:vm'
const mobile = new URL('../', import.meta.url)
const app = await readFile(new URL('../../web/src/App.vue', import.meta.url), 'utf8')
// Silently using ConsoleSession(new Api()) would bypass the native identity boundary.
if (!/props\.api|defineProps<[^>]*api[?]?:\s*Api/s.test(app)) throw new Error('Shared App.vue API injection seam missing. Integrate the shared controller PR before packaging mobile.')
const source = new URL('controller/dist/', mobile)
const html = await readFile(new URL('index.html', source), 'utf8')
const identity = JSON.parse(await readFile(new URL('controller-source.json', source), 'utf8'))
if (identity.format !== 'classic-iife' || !/^assets\/[a-zA-Z0-9_.-]+\.js$/.test(identity.entry)) throw Error('Build one verified classic controller before staging.')
const code = await readFile(new URL(identity.entry, source), 'utf8')
new Script(code)
if (createHash('sha256').update(code).digest('hex') !== identity.sha256 || !html.includes('data-controller-entry="classic"') || /<script[^>]*(?:type=["']module|crossorigin)/.test(html)) throw Error('Controller entry integrity/format mismatch.')
for (const target of ['android/app/src/main/assets/controller/', 'ios/App/controller/']) {
  const destination = new URL(target, mobile)
  await mkdir(destination, { recursive: true })
  await cp(source, destination, { recursive: true, force: true })
  process.stdout.write(`Staged controller: ${fileURLToPath(destination)}\n`)
}
