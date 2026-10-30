import assert from 'node:assert/strict'
import { test } from 'node:test'
import { Native } from './native-process.ts'

function fixture(script: string, budget = 500) { return new Native('/owned-unused', [process.execPath, '-e', script], budget) }
async function reaped(native: Native) {
  try { await native.close() }
  catch (error) { assert.match(String(error), /native driver and hosted trust cleanup must succeed/) }
  assert.ok(native.child.exitCode !== null || native.child.signalCode !== null)
}

test('readiness fences stdin while noisy stderr is drained; a real child then replies and exits', async () => {
  const native = fixture(`
    let ready = false;
    process.stderr.write('MIXYARD_PHASE startup-list-start\\n' + 'unpublished-noise'.repeat(20000));
    setTimeout(() => { ready = true; console.log(JSON.stringify({event:'ready'})) }, 30);
    process.stdin.on('data', () => {
      if (!ready) process.exit(7);
      console.log(JSON.stringify({ok:true,hasAuthority:false}));
    });
    process.stdin.on('end', () => process.exit(0));
  `, 2000)
  try { assert.equal((await native.call({ op: 'pair' })).ok, true) }
  finally { await reaped(native) }
})

test('no readiness times out before sending a request and reaps only the owned process', async () => {
  const native = fixture(`
    process.stderr.write('MIXYARD_PHASE trust-a-start\\n');
    process.stdin.on('data', () => { console.log(JSON.stringify({ok:true,hasAuthority:false})); process.exit(8) });
    setInterval(() => {}, 1000);
  `, 500)
  try { await assert.rejects(native.call({ op: 'pair' }), /native readiness deadline/) }
  finally { await reaped(native) }
  assert.notEqual(native.child.signalCode, null)
})

test('exit before readiness is classified immediately instead of waiting for an RPC timeout', async () => {
  const native = fixture(`process.stderr.write('MIXYARD_PHASE startup-create-failed\\n');process.exit(7)`)
  try { await assert.rejects(native.call({ op: 'pair' }), /Native driver exited \(code 7/) }
  finally { await reaped(native) }
})

test('invalid response fails and reaps the owned actor without revealing raw stderr', async () => {
  const native = fixture(`console.log(JSON.stringify({event:'ready'}));process.stdin.on('data', () => console.log('not-json'))`)
  try { await assert.rejects(native.call({ op: 'pair' }), /invalid or unsolicited JSONL/) }
  finally { await reaped(native) }
})

test('a failed cleanup exit after a successful reply cannot be mistaken for successful shutdown', async () => {
  const native = fixture(`
    console.log(JSON.stringify({event:'ready'}));
    process.stdin.on('data', () => { console.log(JSON.stringify({ok:true,hasAuthority:false})); process.exit(7) });
  `)
  assert.equal((await native.call({op:'pair'})).ok, true)
  if (native.child.exitCode === null) await new Promise(resolve => native.child.once('exit', resolve))
  await assert.rejects(native.close(), /native driver and hosted trust cleanup must succeed/)
})

test('stderr accepts only fixed typed Security classifications and suppresses raw and malformed fields', async () => {
  const output: string[] = []
  const write = process.stderr.write
  process.stderr.write = ((chunk: unknown) => { output.push(String(chunk)); return true }) as typeof write
  const native = fixture(`
    process.stderr.write('SECRET-RAW\\n');
    process.stderr.write('MIXYARD_TRUST_STATE {"phase":"admin-after","absent":true,"statuses":[-25300,-25300,-25263],"resolvedAdminCertificateCount":null,"adminCertificateStatus":-25263}\\n');
    process.stderr.write('MIXYARD_TRUST_STATE {"phase":"admin-after","absent":true,"statuses":[-25300,-25300,-25263],"certificate":"SECRET"}\\n');
    process.stderr.write('MIXYARD_OWNED_SIGNAL {"phase":"before-privileged-term","pid":42,"result":"os-error","errno":1}\\n');
    process.stderr.write('MIXYARD_OWNED_SIGNAL {"phase":"before-privileged-term","pid":42,"result":"os-error","errno":1,"message":"SECRET"}\\n');
    process.stderr.write('MIXYARD_SECURITY {"phase":"trust-a","kind":"timeout","value":4000}\\n');
    process.stderr.write('MIXYARD_SECURITY {"phase":"trust-a","kind":"SECRET-KIND","value":1}\\n');
    process.stderr.write('MIXYARD_SECURITY {"phase":"trust-a","kind":"exit","value":1,"stderr":"SECRET"}\\n');
    process.stderr.write('MIXYARD_SECURITY {"phase":"SECRET","kind":"exit","value":1}\\n');
    console.log(JSON.stringify({event:'ready'}));
    process.stdin.on('data', () => console.log(JSON.stringify({ok:true,hasAuthority:false})));
    process.stdin.on('end', () => process.exit(0));
  `)
  try { await native.ok({op:'pair'}); await reaped(native) }
  finally { process.stderr.write = write; if (native.child.exitCode === null) await reaped(native) }
  assert.match(output.join(''), /Owned Security failure: trust-a timeout 4000/)
  assert.match(output.join(''), /Owned admin trust readback: admin-after absent=true statuses=-25300,-25300,-25263/)
  assert.match(output.join(''), /Owned terminate syscall result \(NOT completion\):/ )
  assert.match(output.join(''), /"errno":1/)
  assert.match(output.join(''), /resolvedAdminCertificateCount=unknown/)
  assert.doesNotMatch(output.join(''), /SECRET|stderr/)
})
