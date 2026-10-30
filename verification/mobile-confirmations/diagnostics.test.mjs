import assert from 'node:assert/strict'
import fs from 'node:fs'
import { test } from 'node:test'
import vm from 'node:vm'
import { attachResourceProbes, resourceProbes } from './resource-probes.mjs'

// Execute the ACTUAL qualification-only embedded observer/query, not a second
// telemetry model. No WebKit, application boot or native dialog is simulated.
const source = fs.readFileSync(new URL('../../apps/mobile/ios/App/MixyardApp.swift', import.meta.url), 'utf8')
const observer = source.match(/ownedObservationScript = """\n([\s\S]*?)\n    """/)[1]
const query = source.match(/web.evaluateJavaScript\("""\n([\s\S]*?)\n        """\)/)[1]
function probe() {
  const events = {}
  class Script { type = 'module'; dataset = {ownedRole:'app'} }
  const context = {HTMLScriptElement: Script, document:{readyState:'complete', getElementById:()=>null, querySelector:()=>null, querySelectorAll:()=>[]}, addEventListener:(name,callback)=>{events[name]=callback}}
  context.window=context
  const realm=vm.createContext(context)
  vm.runInContext(observer,realm)
  return {events, Script, execute:source=>vm.runInContext(source,realm), read:()=>JSON.parse(vm.runInContext(query,realm))}
}
test('runtime evidence allowlists NAME/positions and never leaks message, URL or stack',()=>{
  const {events,read}=probe()
  events.error({target:{},error:{name:'TypeError',message:'SECRET',stack:'SECRET'},message:'SECRET',filename:'/SECRET',lineno:8,colno:71})
  const value=read()
  assert.equal(value.script,'runtime');assert.equal(value.errorName,'TypeError')
  assert.equal(value.line,8);assert.equal(value.column,71);assert.equal(value.boot,'unentered')
  assert.equal(value.capabilities.Response,false);assert.equal(value.capabilities.TextEncoder,false)
  assert.doesNotMatch(JSON.stringify(value),/SECRET/)
})
test('unknown rejection names and invalid positions stay fixed/bounded',()=>{
  const {events,read}=probe()
  events.error({target:{},error:{name:'SECRET'},lineno:-4,colno:Infinity})
  assert.equal(read().line,0);assert.equal(read().column,0)
  events.unhandledrejection({reason:{name:'SECRET',message:'SECRET'}})
  assert.equal(read().script,'runtime');assert.equal(read().errorName,'other') // first error survives
  assert.doesNotMatch(JSON.stringify(read()),/SECRET/)
})
test('module-resource phases contain no asset URL and cannot bootstrap the app',()=>{
  const {events,Script,read}=probe();const script=new Script();script.src='/SECRET'
  events.load({target:script});assert.equal(read().moduleState,'loaded')
  events.error({target:script,filename:'/SECRET'});const value=read()
  assert.equal(value.moduleState,'failed');assert.equal(value.script,'script-resource')
  assert.equal(value.appChildren,0);assert.equal(value.buttons,0)
  assert.doesNotMatch(JSON.stringify(value),/SECRET/)
})

test('tiny probes report only their own resource/execution and never satisfy App readiness',()=>{
  const {events,Script,execute,read}=probe()
  const classic=new Script();classic.type='';classic.dataset.ownedRole='classic-probe'
  execute(resourceProbes[0].source);events.load({target:classic})
  const module=new Script();module.dataset.ownedRole='module-probe';events.error({target:module,error:{name:'SECRET'},filename:'/SECRET'})
  const value=read()
  assert.deepEqual(value.resourceProbes['classic-probe'],{load:'loaded',requestComplete:true,executed:true,errorClass:'none'})
  assert.deepEqual(value.resourceProbes['module-probe'],{load:'failed',requestComplete:true,executed:false,errorClass:'resource-error'})
  assert.equal(value.moduleState,'pending');assert.equal(value.script,'none');assert.equal(value.boot,'unentered');assert.equal(value.appChildren,0);assert.equal(value.buttons,0)
  assert.doesNotMatch(JSON.stringify(value),/SECRET/)
  const app=new Script();events.error({target:app,error:{name:'TypeError'},lineno:1,colno:2})
  events.error({target:module,error:{name:'SECRET'}})
  assert.equal(read().errorRole,'app');assert.equal(read().errorName,'TypeError');assert.equal(read().moduleState,'failed')
})
test('postbuild inserts only same-CSP resource sentinels and preserves actual App module',()=>{
  const csp='<meta http-equiv="Content-Security-Policy" content="script-src self; connect-src none">'
  const app='<script type="module" crossorigin src="./assets/actual-app.js">'
  const source=csp+app+'</script>'
  const value=attachResourceProbes(source)
  assert.ok(value.includes(csp));assert.ok(value.includes('type="module" crossorigin src="./assets/actual-app.js"'))
  for(const probe of resourceProbes)assert.ok(value.includes('src="./'+probe.file+'"'))
  assert.throws(()=>attachResourceProbes(csp));assert.throws(()=>attachResourceProbes(source+app))
})
