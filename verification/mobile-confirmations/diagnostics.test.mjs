import assert from 'node:assert/strict'
import fs from 'node:fs'
import { test } from 'node:test'
import vm from 'node:vm'

// Execute the ACTUAL qualification-only embedded observer/query, not a second
// telemetry model. No WebKit, application boot or native dialog is simulated.
const source = fs.readFileSync(new URL('../../apps/mobile/ios/App/MixyardApp.swift', import.meta.url), 'utf8')
const observer = source.match(/ownedObservationScript = """\n([\s\S]*?)\n    """/)[1]
const query = source.match(/web.evaluateJavaScript\("""\n([\s\S]*?)\n        """\)/)[1]
function probe() {
  const events = {}
  class Script { type = 'module' }
  const context = {HTMLScriptElement: Script, document:{readyState:'complete', getElementById:()=>null, querySelector:()=>null, querySelectorAll:()=>[]}, addEventListener:(name,callback)=>{events[name]=callback}}
  context.window=context
  const realm=vm.createContext(context)
  vm.runInContext(observer,realm)
  return {events, Script, read:()=>JSON.parse(vm.runInContext(query,realm))}
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
  assert.equal(read().script,'rejection');assert.equal(read().errorName,'other')
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
