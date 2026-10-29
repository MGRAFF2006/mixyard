"""Linux real-CLI TLS regression: caller lifetime cannot end mutation ownership.

The preload holds only the owned situation-owner's actual pending-file fsync.
Fixtures, credentials and TLS keys stay private and are removed, never uploaded.
"""
import argparse
import hashlib
import http.client
import json
import os
from pathlib import Path
import signal
import selectors
import socket
import ssl
import subprocess
import tempfile
import time


def read_line(stream):
    value = stream.readline()
    assert value, 'Owned control service ended before readiness'
    return json.loads(value)


class Device:
    def __init__(self, binary, interposer, root):
        self.binary, self.root = binary, root
        root.mkdir(mode=0o700)
        self.state = root / 'identity'
        self.store = root / 'profiles'
        self.store.mkdir(mode=0o700)
        assets = root / 'assets'; assets.mkdir(mode=0o700)
        (assets / 'index.html').write_text('<!doctype html><title>Owned fixture</title>')
        self.trust = self.cli('init', '--ip', '127.0.0.1')
        invitations = [self.cli('invite', '--permissions', roles) for roles in ('view,control', 'view,settings')]
        self.gate, child_gate = socket.socketpair()
        self.gate.settimeout(15)
        environment = dict(os.environ, LD_PRELOAD=str(interposer),
                           MIXYARD_CONTROL_TEST_PENDING=str(self.store / 'pending.json'),
                           MIXYARD_CONTROL_TEST_GATE_FD=str(child_gate.fileno()))
        self.process = subprocess.Popen([str(binary), '--state', str(self.state), 'serve', '--enable-control',
                                         '--bind', '127.0.0.1:0', '--store', str(self.store),
                                         '--assets', str(assets), '--instance', 'owned-completion'],
                                        env=environment, pass_fds=(child_gate.fileno(),),
                                        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        child_gate.close()
        try:
            with selectors.DefaultSelector() as ready:
                ready.register(self.process.stdout, selectors.EVENT_READ)
                assert ready.select(timeout=15), 'Owned control startup deadline exceeded'
                readiness = read_line(self.process.stdout)
            assert readiness['event'] == 'listening' and readiness['tls'] is True
            host, port = readiness['address'].rsplit(':', 1)
            assert host == '127.0.0.1'
            self.address = (host, int(port))
            self.context = ssl.create_default_context(cadata=self.trust['certificatePem'])
            self.context.minimum_version = ssl.TLSVersion.TLSv1_3
            self.writer, self.admin = [self.pair(invite) for invite in invitations]
        except BaseException:
            self.close()
            raise

    def cli(self, *args):
        return json.loads(subprocess.check_output([str(self.binary), '--state', str(self.state), *args], text=True, timeout=15))

    def connection(self):
        connection = http.client.HTTPSConnection(*self.address, context=self.context, timeout=15)
        connection.connect()
        assert hashlib.sha256(connection.sock.getpeercert(binary_form=True)).hexdigest() == self.trust['certificateSha256']
        return connection

    def post(self, path, body, credential=None):
        connection = self.connection()
        try:
            self.send(connection, path, body, credential)
            response = connection.getresponse()
            return response.status, json.loads(response.read())
        finally:
            connection.close()

    @staticmethod
    def send(connection, path, body, credential):
        headers = {'Content-Type': 'application/json'}
        if credential: headers['Authorization'] = 'Bearer ' + credential
        connection.request('POST', path, json.dumps(body, separators=(',', ':')), headers)

    def pair(self, invitation):
        status, reply = self.post('/control/pair', {'deviceId': self.trust['deviceId'],
                                                  'invitation': invitation['invitation'], 'name': 'owned-completion'})
        assert status == 200, 'Owned pairing must succeed'
        return reply

    def session(self, credential):
        return self.post('/control/session', {}, credential)

    def fresh_session(self, credential):
        # Bounded read-only refreshes observe actual completion, not a fake gate
        # acknowledgment. No mutation, revoke or uncertain write is replayed.
        for _ in range(64):
            status, reply = self.session(credential)
            if status != 503: break
            assert reply['error']['code'] == 'service-busy'
        assert status == 200, 'Owner did not complete after the real fsync was released'
        return reply

    def close(self):
        try: self.gate.sendall(b'R')
        except OSError: pass
        self.gate.close()
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
            try: self.process.wait(timeout=25)
            except subprocess.TimeoutExpired:
                self.process.kill(); self.process.wait()
                raise AssertionError('Owned service failed to drain') from None
        assert self.process.returncode == 0, 'Owned service did not exit cleanly'


def envelope(session, body, path=None):
    return {key: session[key] for key in ('deviceId', 'authorityId', 'revision')} | {'body': body} | ({'path': path} if path else {})


def exercise(binary, interposer, root, disconnected):
    device = Device(binary, interposer, root)
    connection = None
    try:
        writer, admin = device.writer['credential'], device.admin['credential']
        status, scope = device.session(writer); assert status == 200
        config = {'schemaVersion': 2, 'revision': 1, 'sources': [], 'mixes': [], 'sends': [], 'outputs': [], 'mediaControls': []}
        write = envelope(scope, {'expectedRevision': 0, 'edit': {'kind': 'save', 'name': 'owned blocked commit', 'config': config}}, '/api/profiles/edit')
        connection = device.connection()
        started = time.monotonic()
        device.send(connection, '/control/request', write, writer)
        assert device.gate.recv(1) == b'F', 'Actual canonical pending-file fsync was not reached'
        assert not (device.store / 'profiles.json').exists()
        if disconnected:
            connection.close(); connection = None
            device.gate.settimeout(9.5)
            try:
                device.gate.recv(1)
                raise AssertionError('Commit gate ended without explicit release')
            except socket.timeout:
                pass
        else:
            response = connection.getresponse()
            assert response.status == 504
            timeout = json.loads(response.read())
            failure = timeout.get('error', timeout.get('body', {}).get('error', {}))
            assert failure['code'] == 'operation-timeout'
            assert 8 <= time.monotonic() - started < 15
            connection.close(); connection = None
        status, busy = device.post('/control/revoke', envelope(scope, {'credentialId': device.writer['credentialId']}), admin)
        assert status == 503 and busy['error']['code'] == 'service-busy', 'Revoke cannot acknowledge before actual commit completion'
        if not disconnected:
            assert 'may still complete' in timeout['error']['message']
        status, busy = device.post('/control/request', write, writer)
        assert status == 503 and busy['error']['code'] == 'service-busy', 'No second write is admitted while the canonical command is active'
        assert not (device.store / 'profiles.json').exists()
        device.gate.sendall(b'R')
        current = device.fresh_session(admin)
        persisted = json.loads((device.store / 'profiles.json').read_text())
        assert current['revision'] == persisted['storeRevision'] == 1
        assert persisted['profiles'][0]['name'] == 'owned blocked commit'
        status, revoked = device.post('/control/revoke', envelope(current, {'credentialId': device.writer['credentialId']}), admin)
        assert status == 200 and revoked['body']['revoked'] is True
        assert device.session(writer)[0] == 401
        assert json.loads((device.store / 'profiles.json').read_text())['storeRevision'] == 1
    finally:
        if connection: connection.close()
        device.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--service', type=Path, required=True)
    parser.add_argument('--interposer', type=Path, required=True)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='mixyard-owned-completion-', dir=Path(tempfile.gettempdir()).resolve()) as directory:
        root = Path(directory)
        exercise(args.service.resolve(), args.interposer.resolve(), root / 'timeout', False)
        exercise(args.service.resolve(), args.interposer.resolve(), root / 'disconnect', True)
    print('PASS: actual canonical commit retains admission beyond HTTP timeout and caller disconnect; revoke follows persistence')
