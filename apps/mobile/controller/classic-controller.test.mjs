import assert from 'node:assert/strict'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'
import { classicController } from './classic-controller.mjs'
const app = fileURLToPath(new URL('../../web/src/App.vue',import.meta.url))
const entry = fileURLToPath(new URL('../../../verification/mobile-confirmations/fixture/main.ts',import.meta.url))
function output(code='(()=>{const text="import( is only display text";})();') {
  return {'assets/app.js':{type:'chunk',fileName:'assets/app.js',isEntry:true,imports:[],dynamicImports:[],exports:[],code,modules:{[app]:{},[entry]:{}}},
    'index.html':{type:'asset',fileName:'index.html',source:'<meta http-equiv="Content-Security-Policy" content="script-src self; connect-src none"><script type="module" crossorigin src="./assets/app.js"></script>'}}
}
function emit(bundle){const files=[];classicController().generateBundle.call({emitFile:value=>files.push(value)},null,bundle);return files}
test('actual classic entry defers without module/CORS attributes and preserves CSP/source hashes',()=>{
 const bundle=output(),files=emit(bundle)
 assert.ok(bundle['index.html'].source.includes('content="script-src self; connect-src none"'))
 assert.match(bundle['index.html'].source,/<script defer data-controller-entry="classic" src=".\/assets\/app.js">/)
 assert.doesNotMatch(bundle['index.html'].source,/type="module"|crossorigin/)
 const record=JSON.parse(files[0].source);assert.equal(record.format,'classic-iife');assert.equal(record.modules.filter(value=>value.role==='shared-app').length,1);assert.match(record.sha256,/^[a-f0-9]{64}$/)
})
test('runtime dynamic import, import.meta and static exports are rejected, display text is not code',()=>{
 for(const code of ['import("./later.js")','console.log(import.meta.url)','export const authority=1']) assert.throws(()=>emit(output(code)))
 assert.doesNotThrow(()=>emit(output()))
})
test('extra chunk, external import/export and missing actual shared source cannot package',()=>{
 for(const field of ['imports','dynamicImports','exports']) {const b=output();b['assets/app.js'][field]=['unexpected'];assert.throws(()=>emit(b))}
 const b=output();b['other.js']={...b['assets/app.js'],isEntry:false};assert.throws(()=>emit(b))
 const missing=output();missing['assets/app.js'].modules={[entry]:{}};assert.throws(()=>emit(missing))
})
test('missing script or extra module loader cannot produce a controller',()=>{
 const b=output();b['index.html'].source='<p>Missing</p>';assert.throws(()=>emit(b))
 const duplicate=output();duplicate['index.html'].source+='<script type="module" src="./extra.js"></script>';assert.throws(()=>emit(duplicate))
})
