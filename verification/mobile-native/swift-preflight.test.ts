import assert from 'node:assert/strict'
import { test } from 'node:test'
import { mkdtempSync, rmSync, realpathSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { Native } from './native-process.ts'
import { certificateFixture } from './certificate-fixture.ts'
test('actual shipping Swift PEM preflight accepts LF and CRLF without leaking rejected diagnostics', {timeout:30_000},async t=>{
 const directory=mkdtempSync(join(realpathSync(tmpdir()),'mixyard-swift-pem-preflight-'))
 let native:Native|undefined,clean=false
 try{
  const cert=certificateFixture(join(directory,'valid'),'valid')
  if(!process.env.MIXYARD_NATIVE_COMMAND)throw Error('Explicit native command required')
  native=new Native(directory,JSON.parse(process.env.MIXYARD_NATIVE_COMMAND))
  for(const ending of ['LF','CRLF'])await t.test(ending,async()=>{
   const pem=ending==='CRLF'?cert.certificatePem.replace(/\n/g,'\r\n'):cert.certificatePem
   const reply=await native!.call({op:'preflightCertificate',deviceId:'0'.repeat(64),trustRecord:JSON.stringify({deviceId:'0'.repeat(64),serverName:'mixyard-control',certificatePem:pem,certificateSha256:cert.pin})})
   console.log('Owned Swift PEM preflight:',ending,'accepted='+reply.ok,'phase='+(['oob-record','oob-certificate','native','tls-platform'].includes(reply.phase??'')?reply.phase:'none'),'reason='+reply.failure)
   assert.equal(reply.ok,true,ending+' certificate preflight must accept the actual valid owned certificate')
  })
  await t.test('unknown associated error data is never exported',async()=>{
   const reply=await native!.call({op:'diagnosticFixture'})
   assert.equal(reply.ok,false);assert.equal(reply.failure,'native-invalid');assert.equal(reply.phase,'native')
   assert.doesNotMatch(JSON.stringify(reply),/PRIVATE_PROVISIONING/)
  })
 }finally{try{await native?.close();clean=true}finally{if(clean)rmSync(directory,{recursive:true,force:true})}}
})
