"""Production native crate -> actual control CLI TLS service; vault is explicitly an API double.
Only harness-owned temporary identity/profile stores and loopback sockets are used.
Never upload the temporary directory: it contains synthetic credentials/private keys.
"""
import argparse
import base64
import contextlib
import hashlib
import json
import os
from pathlib import Path
import queue
import signal
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import time


def receive(stream, timeout=20):
    result = queue.Queue()
    threading.Thread(target=lambda: result.put(stream.readline()), daemon=True).start()
    line = result.get(timeout=timeout)
    assert line, 'Owned process ended before replying'
    return json.loads(line)


class Harness:
    def __init__(self, binary):
        self.targets = {}
        self.process = subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.PIPE, text=True, bufsize=1)

    def call(self, op, **args):
        if op == 'open' and 'credentialId' not in args:
            args['credentialId'] = self.targets[args['id']]['credentialId']
        self.process.stdin.write(json.dumps({'op': op, **args}) + '\n')
        self.process.stdin.flush()
        return receive(self.process.stdout)

    def ok(self, op, **args):
        result = self.call(op, **args)
        assert 'ok' in result, result
        if op == 'pair':
            self.targets[result['ok']['deviceId']] = result['ok']
        return result['ok']

    def close(self):
        self.process.stdin.close()
        self.process.wait(timeout=15)
        assert self.process.returncode == 0, f'Owned native harness exit {self.process.returncode}: ' + self.process.stderr.read()


class Service:
    def __init__(self, binary, root, assets, roles='view,control,settings'):
        self.binary, self.root, self.assets = binary, root, assets
        self.state = root / 'identity'
        self.root.mkdir(mode=0o700)
        self.cli('init', '--ip', '127.0.0.1')
        self.invite = self.cli('invite', '--permissions', roles)
        self.process = None
        self.start()

    def cli(self, *args):
        result = subprocess.run([self.binary, '--state', str(self.state), *args], capture_output=True,
                                text=True, timeout=20, check=True)
        return json.loads(result.stdout)

    def start(self):
        flags = {}
        command = [self.binary, '--state', str(self.state), 'serve', '--enable-control',
                   '--bind', self.ready['address'] if hasattr(self, 'ready') else '127.0.0.1:0',
                   '--store', str(self.root / 'profiles'), '--assets', str(self.assets),
                   '--instance', 'paired-qualification']
        if os.name == 'nt':
            assert os.environ.get('GITHUB_ACTIONS') == 'true', 'Windows console signaling is hosted-owned qualification only'
            startup = subprocess.STARTUPINFO()
            startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
            startup.wShowWindow = 0
            # A separate console contains only this owned service. Never signal the runner/operator console.
            flags = {'creationflags': subprocess.CREATE_NEW_CONSOLE, 'startupinfo': startup}
            command = [sys.executable, str(Path(__file__).with_name('windows-console.py')), *command]
        self.process = subprocess.Popen(command,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, **flags)
        try:
            self.ready = receive(self.process.stdout)
        except (AssertionError, queue.Empty):
            if self.process.poll() is not None:
                raise AssertionError('Owned control service startup failed: ' + self.process.stderr.read()) from None
            raise
        self.endpoint = 'https://' + self.ready['address']

    def stop(self):
        if self.process is not None and self.process.poll() is None:
            if os.name == 'nt':
                helper = '''import ctypes, sys
kernel = ctypes.WinDLL("kernel32", use_last_error=True)
kernel.OpenProcess.argtypes = [ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
kernel.OpenProcess.restype = ctypes.c_void_p
kernel.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
kernel.WaitForSingleObject.restype = ctypes.c_ulong
kernel.CloseHandle.argtypes = [ctypes.c_void_p]
kernel.FreeConsole()
assert kernel.AttachConsole(int(sys.argv[1])), "Cannot attach owned service console"
assert kernel.SetConsoleCtrlHandler(None, True), "Cannot protect owned signal helper"
process = kernel.OpenProcess(0x00100000, False, int(sys.argv[1]))
assert process, "Cannot wait for owned console launcher"
try:
    assert kernel.GenerateConsoleCtrlEvent(0, 0), "Cannot signal owned service Ctrl-C"
    assert kernel.WaitForSingleObject(process, 20000) == 0, "Owned Ctrl-C did not complete service drain"
finally:
    kernel.CloseHandle(process)
'''
                subprocess.run([sys.executable, '-c', helper, str(self.process.pid)], check=True, timeout=22)
            else:
                self.process.send_signal(signal.SIGTERM)
            self.process.wait(timeout=20)
            assert self.process.returncode == 0, f'Owned service exit {self.process.returncode}: ' + self.process.stderr.read()


def envelope(session, path, body):
    return json.dumps({k: session[k] for k in ['deviceId', 'authorityId', 'revision']} | {'path': path, 'body': body}, separators=(',', ':'))


def opened(harness, target):
    reply = harness.ok('open', id=target['deviceId'])
    assert reply['status'] == 200, reply
    identity = json.loads(reply['body'])
    assert identity['deviceId'] == target['deviceId']
    assert identity['backendAvailable'] is False and identity['audioApplied'] is False
    return reply['generation'], identity


def request(harness, target, generation, session, path, body, expected=200):
    reply = harness.ok('request', id=target['deviceId'], generation=generation,
                       raw=envelope(session, path, body))
    assert reply['status'] == expected, reply
    payload = json.loads(reply['body'])
    if 'body' in payload:
        assert payload['deviceId'] == target['deviceId']
        assert payload['authorityId'] == session['authorityId']
        assert payload['status'] == expected
        session['revision'] = payload['revision']
        return payload['body']
    return payload


@contextlib.contextmanager
def tls_fixture(root, identity, handler):
    """Extra adversarial TLS fixtures, never substitutes for the actual CLI service above."""
    data = json.loads((identity / 'state.json').read_text())
    root.mkdir(mode=0o700)
    certificate, key = root / 'cert.pem', root / 'key.pem'
    certificate.write_text(data['certificatePem'])
    encoded = base64.b64encode(bytes(data['privateKeyPkcs8'])).decode()
    key.write_text('-----BEGIN PRIVATE KEY-----\n' + '\n'.join(encoded[i:i+64] for i in range(0, len(encoded), 64)) + '\n-----END PRIVATE KEY-----\n')
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_3
    context.load_cert_chain(certificate, key)
    listener = socket.socket()
    listener.bind(('127.0.0.1', 0)); listener.listen(); listener.settimeout(0.2)
    stop, received = threading.Event(), []
    def serve():
        while not stop.is_set():
            try:
                connection, _ = listener.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            try:
                with context.wrap_socket(connection, server_side=True) as tls:
                    tls.settimeout(3)
                    wire = b''
                    while b'\r\n\r\n' not in wire:
                        piece = tls.recv(8192)
                        if not piece:
                            break
                        wire += piece
                    if not wire:
                        continue
                    head, body = wire.split(b'\r\n\r\n', 1)
                    length = next(int(line.split(b':', 1)[1]) for line in head.split(b'\r\n')[1:] if line.lower().startswith(b'content-length:'))
                    while len(body) < length:
                        body += tls.recv(8192)
                    received.append((head, body))
                    status, payload = handler(head.split(b' ')[1].decode(), body.decode())
                    wire_body = payload.encode()
                    tls.sendall(f'HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {len(wire_body)}\r\nLocation: https://127.0.0.1:1/control/request\r\nConnection: close\r\n\r\n'.encode() + wire_body)
            except (ssl.SSLError, OSError):
                connection.close()
    worker = threading.Thread(target=serve, daemon=True); worker.start()
    try:
        yield f'https://127.0.0.1:{listener.getsockname()[1]}', received
    finally:
        stop.set(); listener.close(); worker.join(timeout=5)
        assert not worker.is_alive()


def qualify(args):
    results = []
    # Resolve only the existing trusted OS temporary base (/var aliases on macOS).
    # Caller-supplied identity paths still go literally through strict admission.
    with tempfile.TemporaryDirectory(prefix='mixyard-desktop-paired-', dir=Path(tempfile.gettempdir()).resolve(strict=True)) as tmp:
        root = Path(tmp)
        harness = Harness(str(Path(args.harness).resolve()))
        services = []
        try:
            a = Service(args.service, root / 'a', args.assets); services.append(a)
            b = Service(args.service, root / 'b', args.assets, 'view'); services.append(b)
            # A failed vault probe must not consume the real one-use invitation.
            harness.ok('vault-fail', fail=True)
            assert 'error' in harness.call('pair', endpoint=a.endpoint, name='A', invitation=a.invite)
            harness.ok('vault-fail', fail=False)
            target_a = harness.ok('pair', endpoint=a.endpoint, name='A', invitation=a.invite)
            target_b = harness.ok('pair', endpoint=b.endpoint, name='B view only', invitation=b.invite)
            assert 'credential' not in target_a and 'credential' not in target_b
            assert len(harness.ok('list')) == 2
            results.append('pair actual rcgen service using narrow OOB trust anchor; failed storage probe preserves invitation; no credential in IPC')
            failure = Service(args.service, root / 'storage-failure', args.assets); services.append(failure)
            harness.ok('vault-write-fail', at=2)  # probe succeeds; credential write after real pairing fails
            failed = harness.call('pair', endpoint=failure.endpoint, name='failed secure write', invitation=failure.invite)
            assert 'error' in failed and 'consumed' in failed['error'].lower()
            harness.ok('vault-write-fail', at=0)
            assert 'error' in harness.call('pair', endpoint=failure.endpoint, name='no invitation replay', invitation=failure.invite)
            assert len(harness.ok('list')) == 2
            failure.stop()
            # Owning fixture inspects only its own private state to revoke the stranded synthetic controller.
            state = json.loads((failure.state / 'state.json').read_text())
            stranded = next(value['credentialId'] for value in state['credentials'].values() if value['name'] == 'failed secure write')
            failure.cli('revoke', '--credential-id', stranded)
            results.append('post-pair secure write failure never claims saved authority; consumed invitation is not replayed; owning fixture revokes stranded controller')
            readback = Service(args.service, root / 'readback-failure', args.assets); services.append(readback)
            harness.ok('vault-readback-fail', id=readback.invite['deviceId'])
            failed = harness.call('pair', endpoint=readback.endpoint, name='failed credential readback', invitation=readback.invite)
            assert 'error' in failed and 'consumed' in failed['error'].lower() and 'revoke' in failed['error'].lower(), failed
            assert 'cleanup completed' in failed['error'], failed
            harness.ok('vault-readback-fail', id=None)
            assert harness.ok('vault-record-present', id=readback.invite['deviceId'])['present'] is False
            assert len(harness.ok('list')) == 2
            assert 'error' in harness.call('pair', endpoint=readback.endpoint, name='no readback invitation replay', invitation=readback.invite)
            readback.stop()
            state = json.loads((readback.state / 'state.json').read_text())
            stranded = next(value['credentialId'] for value in state['credentials'].values() if value['name'] == 'failed credential readback')
            readback.cli('revoke', '--credential-id', stranded)
            results.append('post-pair vault readback error removes unindexed local credential and reports consumed invitation plus owner revocation recovery')
            generation, session = opened(harness, target_a)
            config = json.loads((Path(args.assets).parent / 'src' / 'example-config.json').read_text())
            saved = request(harness, target_a, generation, session, '/api/profiles/edit',
                            {'expectedRevision': session['revision'], 'edit': {'kind': 'draft', 'config': config}})
            assert saved['storeRevision'] == 1
            # B remains independent and view-only; denied write leaves read authority intact.
            gb, sb = opened(harness, target_b)
            request(harness, target_b, gb, sb, '/api/profiles/edit',
                    {'expectedRevision': 0, 'edit': {'kind': 'draft', 'config': config}}, 403)
            assert request(harness, target_b, gb, sb, '/api/profiles/read', {})['storeRevision'] == 0
            assert 'error' in harness.call('revoke', id=target_b['deviceId'], generation=gb)
            assert 'error' in harness.call('open', id=target_b['deviceId'], credentialId='0' * 64)
            request(harness, target_b, gb, sb, '/api/profiles/read', {})
            results.append('stored write on A never changes B; independent view right survives write/revoke denial')
            # Client restart reads persisted vault-double records, service restart invalidates old authority.
            harness.ok('restart')
            assert len(harness.ok('list')) == 2
            generation, session = opened(harness, target_a)
            a.stop(); a.start()
            stale = harness.ok('request', id=target_a['deviceId'], generation=generation,
                               raw=envelope(session, '/api/profiles/read', {}))
            assert stale['status'] == 409
            assert 'error' in harness.call('request', id=target_a['deviceId'], generation=generation,
                                           raw=envelope(session, '/api/profiles/edit', {}))
            generation, fresh = opened(harness, target_a)
            assert fresh['authorityId'] != session['authorityId']
            assert request(harness, target_a, generation, fresh, '/api/profiles/read', {})['storeRevision'] == 1
            results.append('native client restart restores vault-double record; actual service restart preserves write and rejects old authority without replay')
            revoked = harness.ok('revoke', id=target_a['deviceId'], generation=generation)
            assert revoked['status'] == 200
            assert harness.ok('open', id=target_a['deviceId'])['status'] == 401
            a.stop(); a.start()
            assert harness.ok('open', id=target_a['deviceId'])['status'] == 401
            results.append('settings-granted native self-revocation persists across actual service restart')
            # B can remain connected and independently useful after A becomes unreachable/revoked.
            gb, sb = opened(harness, target_b)
            request(harness, target_b, gb, sb, '/api/profiles/read', {})
            harness.ok('disconnect', id=target_b['deviceId'], generation=gb)
            assert 'error' in harness.call('request', id=target_b['deviceId'], generation=gb,
                                           raw=envelope(sb, '/api/profiles/read', {}))
            gb, sb = opened(harness, target_b)
            results.append('native disconnect invalidates stale generation and refresh creates independent authority')
            # Exact pin mismatch is detected before invitation HTTP bytes leave native TLS.
            with tls_fixture(root / 'mismatch', b.state, lambda *_: (500, '{}')) as (endpoint, received):
                invite = dict(a.invite); invite['deviceId'] = 'c' * 64
                assert 'error' in harness.call('pair', endpoint=endpoint, name='wrong TLS certificate', invitation=invite)
                assert received == []
            # Use an owned synthetic TLS responder to prove raw numeric byte preservation and redirect refusal.
            synthetic_id, authority = 'd' * 64, 'e' * 64
            synthetic_invite = dict(b.invite); synthetic_invite['deviceId'] = synthetic_id
            synthetic_invite['permissions'] = ['view', 'settings']
            mode = ['echo']
            def synthetic(route, body):
                if route == '/control/pair':
                    return 200, json.dumps({'deviceId': synthetic_id, 'credentialId': 'f' * 64,
                                             'credential': 'a' * 64, 'permissions': ['view', 'settings']})
                if route == '/control/session':
                    return 200, json.dumps({'deviceId': '0' * 64 if mode[0] == 'wrong-identity' else synthetic_id, 'authorityId': authority, 'revision': 0,
                                             'permissions': ['view', 'settings'], 'backendAvailable': False, 'audioApplied': False})
                if mode[0] == 'redirect':
                    return 307, '{}'
                return 200, '{"deviceId":"' + synthetic_id + '","authorityId":"' + authority + '","revision":0,"status":200,"body":{"value":-0,"finite":1.0000000000000002}}'
            with tls_fixture(root / 'synthetic', b.state, synthetic) as (endpoint, received):
                target = harness.ok('pair', endpoint=endpoint, name='synthetic numeric boundary', invitation=synthetic_invite)
                gs, ss = opened(harness, target)
                raw = envelope(ss, '/api/dsp/preview', {}).replace('"body":{}', '"body":{"value":-0,"finite":1.0000000000000002}')
                reply = harness.ok('request', id=synthetic_id, generation=gs, raw=raw)
                assert received[-1][1] == raw.encode()
                assert '"value":-0,"finite":1.0000000000000002' in reply['body']
                mode[0] = 'redirect'
                assert 'error' in harness.call('request', id=synthetic_id, generation=gs, raw=raw)
                assert len(received) == 4  # pair, session, raw, redirect; no redirect destination call
                mode[0] = 'wrong-identity'
                assert 'error' in harness.call('open', id=synthetic_id)
                harness.ok('forget', id=synthetic_id)
            results.append('exact certificate mismatch before secret HTTP; native raw request/reply -0 and finite bits; redirect denied without retry')
            harness.ok('forget', id=target_a['deviceId'])
            assert [t['deviceId'] for t in harness.ok('list')] == [target_b['deviceId']]
            if args.ui_script:
                ui = Service(args.service, root / 'ui', args.assets); services.append(ui)
                result = subprocess.run(['node', args.ui_script], input=json.dumps({
                    'harness': str(Path(args.harness).resolve()), 'endpoint': ui.endpoint, 'invitation': ui.invite,
                    'cache': str(root / 'owned-vite-cache'), 'screenshot': str(Path(args.output).parent / 'desktop-paired-ui.png'),
                }), capture_output=True, text=True, timeout=90)
                assert result.returncode == 0, result.stderr
                rendered = json.loads(result.stdout)
                assert rendered['status'] == 'passed'
                results.extend(rendered['checks'])
                results.append(rendered['boundary'])
            if os.name == 'nt' or sys.platform == 'darwin':
                roundtrip = harness.ok('platform-vault-roundtrip')
                assert roundtrip['platformVaultRoundtrip'] and roundtrip['reopened'] and roundtrip['deleted']
                results.append('actual hosted platform vault preserves bounded UTF-8 secret bytes across owner reopen and deletes only its owned synthetic entry')
            if os.name != 'nt' and args.expect_no_secret_service:
                harness.ok('platform-vault-unavailable')
                results.append('actual platform vault unavailable fails closed (no production fallback)')
        finally:
            for service in services:
                service.stop()
            harness.close()
    evidence = {'status': 'passed', 'sourceHead': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(), 'checks': results, 'nativeHarnessSha256': hashlib.sha256(Path(args.harness).read_bytes()).hexdigest(),
                'serviceSha256': hashlib.sha256(Path(args.service).read_bytes()).hexdigest(),
                'gaps': ['vault API double persistence is not OS-vault persistence qualification', 'native webview/onboarding GUI and physical devices not exercised',
                         'Secret Service, Windows Credential Manager and macOS Keychain success paths remain separate qualification gates',
                         'no audio, capture, playback, arming, OS defaults, signing, publishing or deployment']}
    Path(args.output).write_text(json.dumps(evidence, indent=2) + '\n')
    print(json.dumps(evidence, indent=2))


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--harness', required=True)
    parser.add_argument('--service', required=True)
    parser.add_argument('--assets', type=Path, required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--expect-no-secret-service', action='store_true')
    parser.add_argument('--ui-script')
    qualify(parser.parse_args())
