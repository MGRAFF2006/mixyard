import assert from 'node:assert/strict'
import test from 'node:test'
import { createRequire } from 'node:module'
import { readFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { pathToFileURL } from 'node:url'
import type { PairedTarget } from './paired.ts'
const require=createRequire(new URL('../../web/package.json',import.meta.url))
const {parse,compileScript}=require('vue/compiler-sfc'), ts=require('typescript')
const vue=require('vue') as typeof import('vue')
function deferred<T>() {let resolve!:(value:T)=>void,reject!:(error:unknown)=>void;const promise=new Promise<T>((ok,fail)=>{resolve=ok;reject=fail});return {promise,resolve,reject}}
const B:PairedTarget={deviceId:'device-B',credentialId:'credential-B',displayName:'B',endpoint:'https://127.0.0.1:1',certificateSha256:'pin',permissions:['view','control']}
const C:PairedTarget={...B,deviceId:'device-C',credentialId:'credential-C',displayName:'C'}
async function scriptComponent(file:string) {
  const historical=file==='App.vue' && process.env.MIXYARD_LIST_BEFORE==='1'
  const source=historical?execFileSync('git',['show','e0dba4a54b6e19ab31244d69deeeb8dedd5b7697:apps/desktop/src/App.vue'],{encoding:'utf8'}):readFileSync(new URL('./'+file,import.meta.url),'utf8')
  const script=compileScript(parse(source,{filename:file}).descriptor,{id:'actual-'+file}).content
  const vueUrl=pathToFileURL(require.resolve('vue/dist/vue.runtime.esm-bundler.js')).href
  // Framework mount/poll scheduling and unrendered child components are boundary
  // doubles; actual App/Pair/Forget bodies + Vue state/nativeInvoke are unchanged.
  const adapter='data:text/javascript;base64,'+Buffer.from('export * from '+JSON.stringify(vueUrl)+';export function onMounted(){};export function onUnmounted(){};').toString('base64')
  const code=ts.transpileModule(script,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText
    .replace(/import (\w+) from ['"][^'"]+\.vue['"];?/g,'const $1 = {};')
    .replace(/from ['"]([^'"]+)['"]/g,(_all:string,path:string)=>'from '+JSON.stringify(path==='vue'?adapter:path.startsWith('.')?new URL(path,new URL('./'+file,import.meta.url)).href:path))
  return (await import('data:text/javascript;base64,'+Buffer.from(code).toString('base64'))).default
}
async function harness() {
  const prior=Object.getOwnPropertyDescriptor(globalThis,'window')
  let saved:PairedTarget[]=[], nextPair=B, lists=0, active=0,maxActive=0
  const answers:Promise<PairedTarget[]>[]=[], starts=new Map<number,ReturnType<typeof deferred<void>>>()
  Object.defineProperty(globalThis,'window',{configurable:true,value:{confirm:()=>true,__TAURI_INTERNALS__:{invoke:async(command:string,args:Record<string,unknown>)=>{
    if(command==='paired_list') {
      const index=lists++;active++;maxActive=Math.max(maxActive,active);starts.get(index)?.resolve()
      try{return await (answers.shift()??Promise.resolve(saved.slice()))}finally{active--}
    }
    if(command==='paired_pair'){saved=[...saved,nextPair];return nextPair}
    if(command==='paired_forget'){saved=saved.filter(target=>target.deviceId!==args.targetId);return}
    if(command==='paired_disconnect'||command==='paired_cancel_open')return
    throw Error('Unexpected owned list-regression command')
  }}}})
  const appComponent=await scriptComponent('App.vue'), childComponent=await scriptComponent('PairedDevices.vue')
  let changedOperation:Promise<void>=Promise.resolve()
  const app=appComponent.setup({}, {expose(){},emit(){}}) as any
  const child=childComponent.setup({targets:[],canRemove:()=>false},{expose(){},emit:(event:string,...args:unknown[])=>{
    if(event==='changed')changedOperation=app.loadTargets()
    if(event==='disconnected')app.disconnectedTarget(...args)
  }}) as any
  const history:string[][]=[];const stop=vue.watch(()=>app.targets.value,()=>history.push(app.paired.value.map((target:PairedTarget)=>target.deviceId)),{flush:'sync'})
  return {app,child,history,answers,starts, stats:()=>({lists,active,maxActive}),setSaved:(value:PairedTarget[])=>{saved=value},
    pair:async(target=B)=>{nextPair=target;child.trusted.value=true;await child.pair();await changedOperation},
    forget:async(target=B)=>{await child.manage(target,'paired_forget');await changedOperation},
    close:()=>{stop();if(prior)Object.defineProperty(globalThis,'window',prior);else Reflect.deleteProperty(globalThis,'window')}}
}

test('delayed startup + actual Pair changed never publishes the pre-pair snapshot',async()=>{
  const h=await harness();try{
    const old=deferred<PairedTarget[]>();h.answers.push(old.promise)
    const pending=h.app.loadTargets();await h.pair(B);old.resolve([C]);await pending
    if(h.app.paired.value.length!==1||h.app.paired.value[0].deviceId!==B.deviceId)console.log('CAUSAL_RED busy-list-lost-pair-notification')
    assert.deepEqual(h.app.paired.value.map((target:PairedTarget)=>target.deviceId),[B.deviceId]);assert.equal(h.stats().lists,2)
    assert.ok(!h.history.some(ids=>ids.includes(C.deviceId)),'pre-notification reply must never publish')
    assert.equal(h.stats().maxActive,1)
  }finally{h.close()}
})
test('held list + actual Forget changed cannot resurrect a removed saved target',async()=>{
  const h=await harness();try{
    h.setSaved([B]);await h.app.loadTargets()
    const old=deferred<PairedTarget[]>();h.answers.push(old.promise)
    const pending=h.app.loadTargets();await h.forget(B);old.resolve([B]);await pending
    if(h.app.paired.value.length)console.log('CAUSAL_RED busy-list-lost-forget-notification')
    assert.equal(h.app.paired.value.length,0);assert.deepEqual(h.app.targets.value.map((target:any)=>target.id),['local'])
    assert.equal(h.stats().lists,3);assert.equal(h.stats().maxActive,1)
  }finally{h.close()}
})
test('multiple changes coalesce once per held read and preserve existing API/codecHost owners', {timeout:2000},async()=>{
  const h=await harness();try{
    h.setSaved([B]);await h.app.loadTargets();const owner=h.app.targets.value[1].api,codecHost={owned:true};h.app.targets.value[1].codecHost=codecHost
    const first=deferred<PairedTarget[]>(),second=deferred<PairedTarget[]>(),secondStarted=deferred<void>();h.answers.push(first.promise,second.promise);h.starts.set(2,secondStarted)
    const pending=h.app.loadTargets();void h.app.loadTargets();void h.app.loadTargets();first.resolve([C]);await secondStarted.promise
    await h.pair(C);void h.app.loadTargets();second.resolve([B]);await pending
    assert.deepEqual(h.app.paired.value.map((target:PairedTarget)=>target.deviceId),[B.deviceId,C.deviceId]);assert.equal(h.stats().lists,4);assert.equal(h.stats().maxActive,1)
    assert.equal(h.app.targets.value[1].api,owner);assert.equal(h.app.targets.value[1].codecHost,codecHost)
    assert.ok(!h.history.some(ids=>ids.length===1&&ids[0]===C.deviceId))
  }finally{h.close()}
})
test('stale list error cannot consume the Pair followup or publish a stale error',async()=>{
  const h=await harness();try{
    const old=deferred<PairedTarget[]>();h.answers.push(old.promise)
    const pending=h.app.loadTargets();await h.pair(B);old.reject(Error('Owned stale list failure'));await pending
    assert.equal(h.app.notice.value,'');assert.equal(h.app.paired.value[0].deviceId,B.deviceId);assert.equal(h.stats().lists,2)
  }finally{h.close()}
})
test('current list error retains owners; a later actual Pair notification still refreshes',async()=>{
  const h=await harness();try{
    h.setSaved([B]);await h.app.loadTargets();const owner=h.app.targets.value[1].api
    h.answers.push(Promise.reject(Error('Owned current list failure')));await h.app.loadTargets()
    assert.match(h.app.notice.value,/Owned current list failure/);assert.equal(h.app.targets.value[1].api,owner)
    await h.pair(C);await h.app.loadTargets();assert.equal(h.app.notice.value,'')
    assert.deepEqual(h.app.paired.value.map((target:PairedTarget)=>target.deviceId),[B.deviceId,C.deviceId]);assert.equal(h.app.targets.value[1].api,owner);assert.equal(h.stats().maxActive,1)
  }finally{h.close()}
})

test('successful followup does not erase an unrelated host notice',async()=>{
  const h=await harness();try{
    const old=deferred<PairedTarget[]>();h.answers.push(old.promise)
    const pending=h.app.loadTargets();await h.pair(B);h.app.notice.value='Owned unrelated tray notice';old.resolve([]);await pending
    assert.equal(h.app.paired.value[0].deviceId,B.deviceId);assert.equal(h.app.notice.value,'Owned unrelated tray notice')
  }finally{h.close()}
})
