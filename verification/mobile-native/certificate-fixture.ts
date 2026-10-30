import { execFileSync } from 'node:child_process'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { X509Certificate, createHash } from 'node:crypto'
export type CertificateFixture = { certificatePem: string; pin: string; key: string }
function openssl(args: string[]) {
 try { execFileSync('openssl',args,{stdio:['ignore','pipe','pipe'],timeout:10_000}) }
 catch { throw Error('Owned OpenSSL certificate fixture preparation failed') } // no argv/paths/keys/errors
}
function date(time: number) {return new Date(time).toISOString().replace(/[-:T]/g,'').replace(/\.\d{3}Z$/,'Z')}
export function certificateFixture(directory: string, mode: 'valid'|'expired'|'future'|'bad-self-signature'|'missing-canonical-name'): CertificateFixture {
 mkdirSync(directory,{mode:0o700})
 const key=join(directory,'key.pem'), seed=join(directory,'seed.pem'), csr=join(directory,'request.pem'), cert=join(directory,'certificate.pem'), config=join(directory,'openssl.cnf')
 openssl(['req','-x509','-newkey','ec','-pkeyopt','ec_paramgen_curve:P-256','-nodes','-keyout',key,'-out',seed,'-days','2','-subj','/CN=Owned certificate fixture'])
 openssl(['req','-new','-key',key,'-out',csr,'-subj','/CN=Owned certificate fixture'])
 writeFileSync(join(directory,'index'),'');writeFileSync(join(directory,'serial'),'1000\n')
 writeFileSync(config,`[ca]
 default_ca=owned
 [owned]
 database=${join(directory,'index')}
 serial=${join(directory,'serial')}
 new_certs_dir=${directory}
 private_key=${key}
 certificate=${seed}
 default_md=sha256
 policy=owned_policy
 x509_extensions=server
 [owned_policy]
 commonName=supplied
 [server]
 basicConstraints=critical,CA:FALSE
 keyUsage=critical,digitalSignature
 extendedKeyUsage=serverAuth
 subjectAltName=IP:127.0.0.1,DNS:localhost${mode==='missing-canonical-name'?'':',DNS:mixyard-control'}
 `)
 const now=Date.now(), day=86_400_000
 const begin=mode==='expired'?now-3*day:mode==='future'?now+day:now-day
 const end=mode==='expired'?now-day:mode==='future'?now+3*day:now+day
 openssl(['ca','-batch','-selfsign','-config',config,'-in',csr,'-out',cert,'-notext','-startdate',date(begin),'-enddate',date(end)])
 let parsed=new X509Certificate(readFileSync(cert)),der=Buffer.from(parsed.raw)
 if(mode==='bad-self-signature'){der.writeUInt8(der.readUInt8(der.length-1)^1,der.length-1);parsed=new X509Certificate(der)}
 if(parsed.verify(parsed.publicKey)!==(mode!=='bad-self-signature')) throw Error('Owned certificate signature fixture invalid')
 const instant=Date.now(),from=Date.parse(parsed.validFrom),until=Date.parse(parsed.validTo)
 if(mode==='expired'?until>=instant:mode==='future'?from<=instant:!(from<=instant&&instant<=until))throw Error('Owned certificate time fixture invalid')
 if(!parsed.checkIP('127.0.0.1'))throw Error('Owned certificate IP fixture invalid')
 return {certificatePem:'-----BEGIN CERTIFICATE-----\n'+der.toString('base64').match(/.{1,64}/g)!.join('\n')+'\n-----END CERTIFICATE-----\n',pin:createHash('sha256').update(der).digest('hex'),key:readFileSync(key,'utf8')}
}
