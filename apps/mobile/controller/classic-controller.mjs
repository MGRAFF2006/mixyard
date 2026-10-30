import { Script } from 'node:vm'
import { createHash } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
const ts = createRequire(new URL('../../web/package.json', import.meta.url))('typescript')

// Vite8/Rolldown IIFE output, no public library name/export. HTML keeps its
// existing origin/CSP; defer preserves module-entry DOM-ready execution order.
export const classicOutput = Object.freeze({ format: 'iife', codeSplitting: false })
export function classicController() {
  return { name: 'mixyard-single-classic-controller', enforce: 'post',
    generateBundle(_options, bundle) {
      const chunks = Object.values(bundle).filter(value => value.type === 'chunk')
      if (chunks.length !== 1 || !chunks[0].isEntry || chunks[0].imports.length || chunks[0].dynamicImports.length || chunks[0].exports.length) throw Error('Controller requires one self-contained entry with no runtime imports/exports.')
      const chunk = chunks[0]
      new Script(chunk.code, {filename:'owned-controller-classic.js'}) // rejects import.meta/static imports/exports
      const syntax = ts.createSourceFile('controller.js', chunk.code, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS)
      const inspect = node => {
        if (ts.isCallExpression(node) && node.expression.kind === ts.SyntaxKind.ImportKeyword) throw Error('Controller cannot contain dynamic import.')
        ts.forEachChild(node, inspect)
      }
      inspect(syntax)
      const html = Object.values(bundle).filter(value => value.type === 'asset' && value.fileName === 'index.html')
      if (html.length !== 1 || typeof html[0].source !== 'string') throw Error('Controller index is required.')
      const source = html[0].source
      const tags = [...source.matchAll(/<script\b[^>]*>\s*<\/script>/g)]
      if (tags.length !== 1 || !tags[0][0].includes(chunk.fileName) || !/\btype="module"/.test(tags[0][0])) throw Error('Unexpected controller script entry.')
      const classic = tags[0][0].replace(/\s+type="module"/, '').replace(/\s+crossorigin(?:="[^"]*")?/, '').replace('<script', '<script defer data-controller-entry="classic"')
      html[0].source = source.replace(tags[0][0], classic)
      if (/<script[^>]*type=["']module/.test(html[0].source) || /rel=["']modulepreload/.test(html[0].source)) throw Error('Unexpected remaining module loader.')
      const hash = data => createHash('sha256').update(data).digest('hex')
      const modules = Object.keys(chunk.modules).filter(name => !name.startsWith('\0') && !name.includes('/node_modules/') && /\.(?:vue|ts|css)$/.test(name))
        .map(name => ({ role: name.endsWith('/apps/web/src/App.vue') ? 'shared-app' : name.endsWith('/main.ts') ? 'entry' : 'shared-source', sha256: hash(readFileSync(name.split('?')[0])) }))
      if (!modules.some(value => value.role === 'shared-app') || !modules.some(value => value.role === 'entry')) throw Error('Actual shared App and entry source identity required.')
      this.emitFile({type:'asset',fileName:'controller-source.json',source:JSON.stringify({format:'classic-iife',entry:chunk.fileName,sha256:hash(chunk.code),modules},null,2)})
    },
  }
}
