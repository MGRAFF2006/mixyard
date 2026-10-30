import assert from 'node:assert/strict'
import { test } from 'node:test'
import { mkdtempSync, rmSync, realpathSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { once } from 'node:events'
import { createServer } from 'node:https'
import { createHash } from 'node:crypto'
import { Native } from './native-process.ts'
import { certificateFixture, type CertificateFixture } from './certificate-fixture.ts'
const required=(name:string)=>{if(!process.env[name])throw Error('Explicit owned native command required');return process.env[name]!}
// All acceptance assertions exercise SHIPPING PinnedControl/ControlClient and
// public platform PKI, never a JavaScript trust implementation or global store.
test('shipping single-OOB-anchor public SSL policy rejects invalid certificates before HTTP', {timeout:180_000},async t=>{
 const directory=mkdtempSync(join(realpathSync(tmpdir()),'mixyard-owned-public-policy-'))
 const command=JSON.parse(required('MIXYARD_NATIVE_COMMAND')) as string[]
 let native:Native|undefined,clean=false
 try{
  const valid=certificateFixture(join(directory,'valid'),'valid')
  const fixtures=new Map<string,CertificateFixture>((['expired','future','bad-self-signature','missing-canonical-name'] as const).map(mode=>[mode,certificateFixture(join(directory,mode),mode)]))
  const other=certificateFixture(join(directory,'other'),'valid')
  native=new Native(directory,command)
  for(const mode of ['expired','future','bad-self-signature','wrong-oob-pin','wrong-target-anchor','tls-signature-algorithm','cross-device','duplicate-field','escaped-duplicate-field','unknown-field','oversize-record','wrong-name-tuple','trailing-json','nonstring-field','missing-canonical-name','multiple-certificates','malformed-der','tls12-only'] as const)await t.test(mode,async()=>{
   const serverCert=fixtures.get(mode)??valid
   let requests=0,connections=0
   const server=createServer({cert:serverCert.certificatePem,key:serverCert.key,minVersion:mode==='tls12-only'?'TLSv1.2':'TLSv1.3',maxVersion:mode==='tls12-only'?'TLSv1.2':'TLSv1.3',...(mode==='tls-signature-algorithm'?{sigalgs:'rsa_pss_rsae_sha256'}:{})},(req,res)=>{requests++;req.resume();res.writeHead(500);res.end()})
   server.on('connection',()=>{connections++});server.on('tlsClientError',()=>{})
   const sockets=new Set<import('node:stream').Duplex>();server.on('connection',socket=>{sockets.add(socket);socket.on('close',()=>sockets.delete(socket))})
   try{
    server.listen(0,'127.0.0.1');await once(server,'listening')
    const oob=mode==='wrong-target-anchor'?other:serverCert
    const record={deviceId:'0'.repeat(64),serverName:'mixyard-control',certificatePem:oob.certificatePem,certificateSha256:mode==='wrong-oob-pin'?other.pin:oob.pin}
    let trustRecord=JSON.stringify(record)
    if(mode==='duplicate-field')trustRecord=trustRecord.slice(0,-1)+',"deviceId":"'+record.deviceId+'"}'
    if(mode==='escaped-duplicate-field')trustRecord=trustRecord.slice(0,-1)+String.raw`,"\u0064eviceId":"${record.deviceId}"}`
    if(mode==='unknown-field')trustRecord=trustRecord.slice(0,-1)+',"unknown":""}'
    if(mode==='oversize-record')trustRecord=' '.repeat(65537)+trustRecord
    if(mode==='wrong-name-tuple')trustRecord=JSON.stringify({...record,serverName:'unrelated'})
    if(mode==='trailing-json')trustRecord+=' null'
    if(mode==='nonstring-field')trustRecord=JSON.stringify({...record,deviceId:false})
    if(mode==='multiple-certificates')trustRecord=JSON.stringify({...record,certificatePem:record.certificatePem+other.certificatePem})
    if(mode==='malformed-der'){const der=Buffer.from([0x30,0]);trustRecord=JSON.stringify({...record,certificatePem:'-----BEGIN CERTIFICATE-----\n'+der.toString('base64')+'\n-----END CERTIFICATE-----\n',certificateSha256:createHash('sha256').update(der).digest('hex')})}
    const result=await native!.call({op:'pair',endpoint:`https://127.0.0.1:${(server.address() as import('node:net').AddressInfo).port}`,trustRecord,deviceId:(mode==='cross-device'?'1':'0').repeat(64),invitation:'owned-negative-must-not-dispatch'})
    console.log('Owned public SDK negative:',mode,'httpRequests='+requests,'tlsConnections='+connections,'nativeAccepted='+result.ok)
    assert.equal(requests,0,mode+' must reject BEFORE invitation HTTP')
    assert.equal(result.ok,false,mode+' must fail closed')
   }finally{for(const socket of sockets)socket.destroy();await new Promise<void>(ok=>server.close(()=>ok()))}
  })
  await t.test('existing system certificate evaluator negative is explicitly NOT external wire proof',async()=>{
   const reply=await native!.call({op:'evaluateSystemCertificate',trustRecord:JSON.stringify({deviceId:'0'.repeat(64),serverName:'mixyard-control',certificatePem:valid.certificatePem,certificateSha256:valid.pin}),deviceId:'0'.repeat(64)})
   assert.equal(reply.ok,true,'public system certificate evaluation must actually be available')
   assert.equal(reply.systemCertificateObservedTrusted,true);assert.equal(reply.privateAnchorRejected,true);assert.equal(reply.evaluatorOnly,true)
   console.log('Owned public SDK evaluator ONLY: existing system certificate observed trusted, private-anchor provider rejected; NO external leaf/HTTP proof')
  })
  await t.test('legacy missing anchor cannot send credential HTTP and retains its record',async()=>{
   let requests=0
   const server=createServer({cert:valid.certificatePem,key:valid.key},(req,res)=>{requests++;req.resume();res.writeHead(500);res.end()})
   try{
    server.listen(0,'127.0.0.1');await once(server,'listening')
    const setup=await native!.call({op:'installLegacyFixture',endpoint:`https://127.0.0.1:${(server.address() as import('node:net').AddressInfo).port}`,pin:valid.pin,deviceId:'0'.repeat(64)})
    assert.equal(setup.ok,true);assert.equal(setup.recordRetained,true)
    const reply=await native!.call({op:'invoke',method:'session',payload:'{}'})
    assert.equal(reply.ok,false);assert.equal(reply.recordRetained,true);assert.equal(requests,0,'legacy credential cannot leave native storage without an OOB anchor')
   }finally{server.closeAllConnections();await new Promise<void>(ok=>server.close(()=>ok()))}
  })
 }finally{
  try{await native?.close();clean=true}finally{if(clean)rmSync(directory,{recursive:true,force:true})}
 }
})
