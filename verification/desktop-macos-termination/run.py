"""Owned hosted macOS AppKit probe + separately started real runtime survival."""
import hashlib
import json
import os
from pathlib import Path
import platform
import socket
import stat
import struct
import subprocess
import sys
import time


def fingerprint(path):
    with path.open('rb') as stream:
        return {'path': str(path), 'sha256': hashlib.file_digest(stream, 'sha256').hexdigest()}


def main(probe, runtime, cli, output):
    if platform.system() != 'Darwin' or os.environ.get('GITHUB_ACTIONS') != 'true':
        raise SystemExit('Use the dedicated disposable hosted macOS workflow; never the operator desktop')
    output.mkdir(exist_ok=True, parents=True)
    instance = f'mac-quit-{os.getpid()}'
    endpoint = Path(f'/private/tmp/mixyard-{os.getuid()}-{instance}/runtime.sock')
    assert not endpoint.parent.exists(), 'Refuse an existing runtime namespace'
    environment = {key: value for key, value in os.environ.items() if key not in {
        'DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'DBUS_SYSTEM_BUS_ADDRESS',
        'PULSE_SERVER', 'PIPEWIRE_REMOTE'}}
    report = {'status': 'failed', 'platform': platform.platform(), 'machine': platform.machine(),
              'binaries': [fingerprint(path) for path in [probe, runtime, cli]], 'steps': [],
              'boundary': 'Actual AppKit menu/terminate and production delegate shim; separate shared Chromium child-draft tests. Not a full Tauri WKWebView, physical Dock/Cmd+Q, accessibility, installer or audio proof.'}
    child = None
    directory_identity = None
    def request(command, ready=False):
        reply = subprocess.run([str(cli), command, '--instance', instance], env=environment,
                               capture_output=True, text=True, timeout=5)
        if ready and reply.returncode:
            return None
        assert reply.returncode == 0, reply.stderr
        return json.loads(reply.stdout)['result']
    def survival():
        assert child.poll() is None, 'Harness-owned runtime exited'
        assert stat.S_IMODE(endpoint.parent.stat().st_mode) == 0o700
        assert stat.S_IMODE(endpoint.stat().st_mode) == 0o600
        assert endpoint.stat().st_uid == os.getuid()
        with socket.socket(socket.AF_UNIX) as peer:
            peer.settimeout(3); peer.connect(str(endpoint))
            # Darwin sys/un.h: SOL_LOCAL=0, LOCAL_PEERPID=2; kernel process ID.
            peer_pid = struct.unpack('i', peer.getsockopt(0, 2, 4))[0]
        assert peer_pid == child.pid, (peer_pid, child.pid)
        status = request('status')
        assert status['backendAvailable'] is False and status['nativeAudioProof'] == 'unverified'
        assert all(status[key] is None for key in ['audioRevision', 'appliedRevision', 'desiredRevision'])
        return {'runtime_pid': child.pid, 'peer_pid': peer_pid, 'status': status}
    try:
        with output.joinpath('runtime.log').open('w') as log:
            child = subprocess.Popen([str(runtime), 'serve', '--instance', instance], env=environment,
                                     stdout=log, stderr=log)
        deadline = time.monotonic() + 10
        while not request('status', ready=True):
            assert child.poll() is None and time.monotonic() < deadline, 'Runtime readiness failed'
            time.sleep(.05)
        directory_identity = endpoint.parent.stat()
        report['steps'].append({'action': 'runtime-ready', **survival()})
        for mode in ['--without-guard', '--guarded']:
            result = subprocess.run([str(probe), mode], env={**environment, 'MIXYARD_MACOS_TERMINATION_PROBE': '1'},
                                    capture_output=True, text=True, timeout=20)
            output.joinpath(f'{mode[2:]}.stdout').write_text(result.stdout)
            output.joinpath(f'{mode[2:]}.stderr').write_text(result.stderr)
            assert result.returncode == 0, result.stderr
            events = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
            if mode == '--without-guard':
                assert [event['event'] for event in events] == ['native-will-terminate'], events
                assert events[0]['guarded'] is False
            else:
                assert [event['event'] for event in events] == ['cancelled', 'cancelled', 'approved-ui-exit'], events
                assert [event['origin'] for event in events[:2]] == ['menu-terminate', 'dock-equivalent-terminate']
                assert len({(event['pid'], event['window']) for event in events}) == 1, events
            report['steps'].append({'action': mode[2:], 'events': events, **survival()})
        assert request('shutdown')['shutdown'] == 'requested'
        assert child.wait(timeout=10) == 0
        report['status'] = 'passed'
    except Exception as error:
        report['failure'] = repr(error)
        raise
    finally:
        output.joinpath('macos-termination.json').write_text(json.dumps(report, indent=2) + '\n')
        if child is not None and child.poll() is None:
            child.terminate(); child.wait(timeout=5)
        if directory_identity and endpoint.parent.exists():
            current = endpoint.parent.stat()
            if (current.st_dev, current.st_ino) == (directory_identity.st_dev, directory_identity.st_ino):
                try:
                    endpoint.parent.rmdir()
                except OSError as error:
                    print(f'Owned runtime namespace retained: {error}', file=sys.stderr)



if __name__ == '__main__':
    main(*(Path(arg).resolve() for arg in sys.argv[1:5]))
