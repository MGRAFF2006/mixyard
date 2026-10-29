import test from 'node:test';
import assert from 'node:assert/strict';
import { Boundary, endpoint, snapshot } from '../../mixyard_interop/public_sdk/boundary.mjs';

test('metadata traversal is bounded before serialization and refuses unsafe u64 rounding', () => {
  assert.throws(() => snapshot({ stream: 2 ** 53 }), /safe integers/);
  assert.throws(() => snapshot({ channels: new Array(129).fill('x') }), /collection/);
  let nested = {};
  for (let i = 0; i < 14; i++) nested = { nested };
  assert.throws(() => snapshot(nested), /budget/);
  assert.throws(() => snapshot({ provenance: 'x'.repeat(4097) }), /budget/);
  assert.throws(() => snapshot({ value: NaN }), /safe integers/);
});

test('loopback endpoint validation rejects coerced ports and alternative host spellings', () => {
  for (const host of ['localhost', '127.1', '2130706433', '::1', '127.0.0.2']) assert.throws(() => endpoint({ host, port: 1234 }));
  for (const port of [0, 65536, true, '1234']) assert.throws(() => endpoint({ host: '127.0.0.1', port }));
});

test('missing or asynchronous authorization is never a grant', () => {
  const options = { endpoint: { host: '127.0.0.1', port: 1234 }, identity: 'peer' };
  for (const authorize of [undefined, async () => true]) {
    let cleaned = false;
    const policy = new Boundary({ ...options, authorize }, () => { cleaned = true; });
    assert.throws(() => policy.check('control', 'sdk-command'), /denied/);
    assert(cleaned);
  }
});

test('synchronous host revocation overrides a true authorization result', () => {
  let policy;
  let cleaned = 0;
  policy = new Boundary({ endpoint: { host: '127.0.0.1', port: 1234 }, identity: 'peer',
    authorize: () => { policy.close(); return true; } }, () => { cleaned++; });
  assert.throws(() => policy.check('control', 'effect'), /closed/);
  assert.equal(cleaned, 1);
});
