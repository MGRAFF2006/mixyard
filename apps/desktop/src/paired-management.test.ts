import assert from 'node:assert/strict'
import test from 'node:test'
import { createRequire } from 'node:module'
import { readFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { pathToFileURL } from 'node:url'
import { pairedApi } from './paired.ts'
import { ConsoleSession } from '../../web/src/session.ts'
import { readConfig, signature } from '../../web/src/model.ts'
import type { Invoke } from './transport.ts'
const require=createRequire(new URL('../../web/package.json',import.meta.url))
const vue=require('vue') as typeof import('vue')
const {parse,compileScript}=require('vue/compiler-sfc')
const ts=require('typescript')

test('actual PairedDevices.manage invalidates captured authority after failed pending Forget; drafts survive',async()=>{
  const target={deviceId:'device-A',credentialId:'credential-A',displayName:'A',endpoint:'https://127.0.0.1:1',certificateSha256:'pin',permissions:['view','control'] as ('view'|'control')[]}
  const identity={deviceId:target.deviceId,authorityId:'owned-A',revision:0,permissions:target.permissions,backendAvailable:false,audioApplied:false}
  const config=readConfig(JSON.parse(readFileSync(new URL('../../web/src/example-config.json',import.meta.url),'utf8')))
  const store={format:'mixyard-local-drafts',schemaVersion:1,storeRevision:0,configRevision:1,profiles:[],draft:config,previousDraft:null}
  let generation=0,serial=0,nativeConnected=false,requests=0,release!:()=>void,arrived!:()=>void
  const held=new Promise<void>(resolve=>{release=resolve}),pending=new Promise<void>(resolve=>{arrived=resolve})
  const invoke: Invoke=async<T>(command:string,args?:Record<string,unknown>)=>{
    if(command==='paired_prepare_open')return {generation,serial:++serial} as T
    if(command==='paired_open') {assert.equal((args!.intent as {generation:number}).generation,generation);nativeConnected=true;return {generation:++generation,status:200,body:JSON.stringify(identity)} as T}
    if(command==='paired_disconnect') {if(args!.expectedGeneration===generation){generation++;nativeConnected=false}return undefined as T}
    if(command==='paired_forget') {generation++;nativeConnected=false;arrived();await held;generation++;nativeConnected=false;throw 'Owned vault delete failed after final native invalidation'}
    assert.equal(command,'paired_request');assert.equal(nativeConnected,true);assert.equal(args!.generation,generation);requests++
    const path=JSON.parse(args!.raw as string).path
    const body=path==='/api/profiles/read'?store:path==='/api/status'?{connected:false,snapshot:null,error:null}:{valid:true}
    return {status:200,body:JSON.stringify({...identity,status:200,body})} as T
  }
  const previousWindow=Object.getOwnPropertyDescriptor(globalThis,'window')
  Object.defineProperty(globalThis,'window',{configurable:true,value:{__TAURI_INTERNALS__:{invoke},confirm:()=>true}})
  const api=pairedApi(invoke,target),workspace=new ConsoleSession(api)
  const stop=vue.watch(()=>api.epoch,()=>workspace.invalidate(),{flush:'sync'})
  const events:unknown[][]=[]
  try {
    const historical=process.env.MIXYARD_MANAGEMENT_BEFORE==='1'
    const source=historical?execFileSync('git',['show','f6b1b0df5c31aef975875098b8a9a385c61aa899:apps/desktop/src/PairedDevices.vue'],{encoding:'utf8'}):readFileSync(new URL('./PairedDevices.vue',import.meta.url),'utf8')
    const script=compileScript(parse(source,{filename:'PairedDevices.vue'}).descriptor,{id:'actual-management-regression'}).content
    const code=ts.transpileModule(script,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ESNext}}).outputText
      .replace(/from ['"]vue['"]/g,'from '+JSON.stringify(pathToFileURL(require.resolve('vue/dist/vue.runtime.esm-bundler.js')).href))
      .replace(/from ['"]\.\/bridge\.ts['"]/g,'from '+JSON.stringify(new URL('./bridge.ts',import.meta.url).href))
    const component=(await import('data:text/javascript;base64,'+Buffer.from(code).toString('base64'))).default
    const props={targets:[target],canRemove:()=>false}
    // Actual compiled shipping setup/management body + actual Vue reactivity.
    // Only invoke scheduling/native results and component event delivery double.
    const state=component.setup(props,{expose(){},emit:(...event:unknown[])=>{
      events.push(event)
      if(event[0]==='disconnected' && event[1]===target.deviceId && (event[2]===undefined || event[2]===target.credentialId))api.disconnect()
    }}) as {manage:(target:unknown,command:string)=>Promise<void>}
    await workspace.refresh();workspace.local!.sends[0]!.gain=0.37;workspace.changed()
    const management=state.manage(target,'paired_forget');await pending
    await workspace.refresh();await workspace.validate()
    assert.equal(workspace.canWrite,true);assert.equal(workspace.validated,true)
    const draft=signature(workspace.local)
    props.targets=[] // Settlement still belongs to the captured original credential.
    release();await management
    if(api.connected && workspace.canWrite && workspace.validated)console.log('CAUSAL_RED failed-forget-left-shared-replacement-authority')
    assert.equal(nativeConnected,false);assert.equal(api.connected,false);assert.equal(api.identity,null)
    assert.equal(workspace.canWrite,false);assert.equal(workspace.validated,false);assert.equal(signature(workspace.local),draft)
    const count=requests;await assert.rejects(api.post('/api/profiles/write'),/session unavailable|Refresh to reconnect/i);assert.equal(requests,count)
    assert.deepEqual(events.filter(value=>value[0]==='disconnected').at(-1),['disconnected',target.deviceId,target.credentialId])
  } finally {release();stop();api.disconnect();if(previousWindow)Object.defineProperty(globalThis,'window',previousWindow);else Reflect.deleteProperty(globalThis,'window')}
})
