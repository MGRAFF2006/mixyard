import assert from 'node:assert/strict'
import { test } from 'node:test'
import { mkdtempSync, rmSync, realpathSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { X509Certificate, createPrivateKey, createPublicKey } from 'node:crypto'
import { certificateFixture } from './certificate-fixture.ts'
test('owned public-policy fixtures have causal time/signature defects and matching TLS key',()=>{
 const root=mkdtempSync(join(realpathSync(tmpdir()),'mixyard-owned-cert-fixtures-'))
 try{for(const mode of ['valid','expired','future','bad-self-signature','missing-canonical-name'] as const){
  const f=certificateFixture(join(root,mode),mode),cert=new X509Certificate(f.certificatePem)
  assert.equal(cert.verify(cert.publicKey),mode!=='bad-self-signature')
  assert.ok(cert.publicKey.equals(createPublicKey(createPrivateKey(f.key))))
  assert.equal(cert.checkIP('127.0.0.1'),'127.0.0.1')
  assert.equal(cert.checkHost('mixyard-control'),mode==='missing-canonical-name'?undefined:'mixyard-control')
 }}finally{rmSync(root,{recursive:true,force:true})}
})
