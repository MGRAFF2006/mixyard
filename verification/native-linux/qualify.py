#!/usr/bin/env python3
"""Read-only laptop discovery. Never launches an audio producer or mutates a graph."""
import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import resource
import signal
import subprocess
import tempfile
import threading
import time

LIMIT = 8 * 1024 * 1024
INTERFACES = frozenset('Core Registry Module Device Node Port Link Client Factory Metadata '
                       'Profiler Session Endpoint EndpointStream EndpointLink'.split())
INTERFACE_NAMES = frozenset('PipeWire:Interface:' + name for name in INTERFACES)
DIRECTORY_FLAGS = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC


class CaptureCancelled(Exception):
    """The owned process group has been stopped before cancellation is reported."""


def strict_json(data):
    def object_pairs(pairs):
        value = {}
        for key, item in pairs:
            if key in value:
                raise ValueError('duplicate JSON key')
            value[key] = item
        return value

    def invalid_constant(_):
        raise ValueError('nonfinite JSON number')

    if len(data) >= LIMIT:
        raise ValueError('input bound exceeded')
    try:
        return json.loads(data, object_pairs_hook=object_pairs, parse_constant=invalid_constant)
    except RecursionError:
        raise ValueError('JSON nesting exceeds decoder limit') from None


@contextmanager
def evidence_directory(root, output):
    """Pin each directory; never follow links into an unignored destination."""
    root = Path(root).resolve()
    allowed = root / 'verification' / 'local-evidence'
    output = Path(os.path.abspath(output))
    try:
        parts = output.relative_to(allowed).parts
    except ValueError:
        raise ValueError('output must be beneath verification/local-evidence') from None
    if not parts:
        raise ValueError('output must be a new evidence subdirectory')
    opened = []
    try:
        opened.append(os.open(root, DIRECTORY_FLAGS))
        opened.append(os.open('verification', DIRECTORY_FLAGS, dir_fd=opened[-1]))
        for index, name in enumerate(('local-evidence', *parts)):
            final = index == len(parts)
            try:
                os.mkdir(name, mode=0o700, dir_fd=opened[-1])
            except FileExistsError:
                if final:
                    raise ValueError('output directory must be new') from None
            fd = os.open(name, DIRECTORY_FLAGS, dir_fd=opened[-1])
            opened.append(fd)
            metadata = os.fstat(fd)
            if metadata.st_uid != os.geteuid() or metadata.st_mode & 0o777 != 0o700:
                raise ValueError('evidence directory must be owned and private')
        yield opened[-1]
    finally:
        for fd in reversed(opened):
            os.close(fd)


@contextmanager
def private_file(path, directory_fd=None):
    fd = os.open(path, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,
                 0o600, dir_fd=directory_fd)
    with os.fdopen(fd, 'w+b') as stream:
        metadata = os.fstat(stream.fileno())
        if (metadata.st_uid != os.geteuid() or metadata.st_mode & 0o777 != 0o600
                or metadata.st_nlink != 1):
            raise ValueError('evidence file must be owned, private and unlinked elsewhere')
        yield stream


def run_owned(argv, output, error, timeout, cwd=None):
    """Bound and reap a new process group while its leader PID stays pinned."""
    if threading.current_thread() is not threading.main_thread():
        raise ValueError('capture requires the owning main thread')
    if signal.getsignal(signal.SIGCHLD) != signal.SIG_DFL:
        raise ValueError('capture requires exclusive child reaping')
    cancelled = False

    def cancel(_signum, _frame):
        nonlocal cancelled
        cancelled = True

    def bounds():
        resource.setrlimit(resource.RLIMIT_FSIZE, (LIMIT, LIMIT))
        os.umask(0o077)

    previous = {sig: signal.signal(sig, cancel) for sig in (signal.SIGINT, signal.SIGTERM)}
    child = None
    try:
        child = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=output, stderr=error,
                                 cwd=cwd, start_new_session=True, preexec_fn=bounds)
        deadline = time.monotonic() + timeout
        while True:
            if cancelled:
                raise CaptureCancelled()
            if time.monotonic() >= deadline:
                raise subprocess.TimeoutExpired('owned discovery', timeout)
            # WNOWAIT leaves even an exited leader unreaped until the group is
            # killed, so its PID cannot be reused to target an unrelated group.
            result = os.waitid(os.P_PID, child.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
            if result is not None:
                break
            time.sleep(0.01)
    finally:
        try:
            if child is not None:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.wait(timeout=1)
        finally:
            for sig, handler in previous.items():
                signal.signal(sig, handler)
    if cancelled:
        raise CaptureCancelled()
    return child.returncode


def capture(argv, path, *, directory_fd=None, timeout=12, cwd=None, decode=True, artifacts=None):
    path = Path(path)
    stderr_path = path.with_suffix('.stderr')
    with private_file(path, directory_fd) as output, private_file(stderr_path, directory_fd) as error:
        try:
            status = run_owned(argv, output, error, timeout, cwd)
            if status or os.fstat(error.fileno()).st_size or os.fstat(output.fileno()).st_size >= LIMIT:
                raise ValueError('command failed or exceeded output bound')
            output.seek(0)
            data = output.read(LIMIT)
            return strict_json(data) if decode else data
        finally:
            if artifacts is not None:
                for name, stream in ((path.name, output), (stderr_path.name, error)):
                    stream.seek(0)
                    artifacts[name] = hashlib.sha256(stream.read(LIMIT)).hexdigest()


def inventory_summary(value):
    if not isinstance(value, list) or not 1 <= len(value) <= 8192:
        raise ValueError('expected bounded nonempty pw-dump object array')
    ids = set()
    counts = {}
    for item in value:
        if (not isinstance(item, dict) or type(item.get('id')) is not int
                or not 0 <= item['id'] < 2**32):
            raise ValueError('invalid object identity')
        if item['id'] in ids:
            raise ValueError('duplicate object identity')
        ids.add(item['id'])
        kind = item.get('type')
        if not isinstance(kind, str) or kind not in INTERFACE_NAMES:
            raise ValueError('unrecognized object type')
        counts[kind] = counts.get(kind, 0) + 1
    if counts.get('PipeWire:Interface:Core') != 1:
        raise ValueError('expected exactly one observed core')
    return counts


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True,
                        help='new directory beneath verification/local-evidence')
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    try:
        # Pin/reject unsafe evidence paths before any native discovery. The
        # selected relative path must also actually be ignored by this checkout.
        with evidence_directory(root, args.output) as directory_fd:
            with tempfile.TemporaryDirectory(prefix='mixyard-host-') as temp:
                relative = Path(os.path.abspath(args.output)).relative_to(root)
                capture(['/usr/bin/git', 'check-ignore', '--quiet', '--', str(relative)],
                        Path(temp) / 'ignored', cwd=root, decode=False)
                host = capture(['/usr/bin/tailscale', 'status', '--json'], Path(temp) / 'host.json')
                if (not isinstance(host, dict) or not isinstance(host.get('Self'), dict)
                        or host['Self'].get('HostName') != 'laptop'):
                    raise ValueError('not the authorized laptop')
                source = capture(['/usr/bin/git', 'rev-parse', 'HEAD'], Path(temp) / 'source',
                                 cwd=root, decode=False).decode('ascii').strip()
                if re.fullmatch(r'[a-f0-9]{40}|[a-f0-9]{64}', source) is None:
                    raise ValueError('invalid source revision')
            report = {'schemaVersion': 1, 'label': 'native-read-only-metadata',
                      'host': 'laptop', 'capturedAt': datetime.now(timezone.utc).isoformat(),
                      'checkerSha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                      'sourceSha': source, 'claims': [], 'gaps': [
                          'Snapshots do not establish complete inventory or stable selectors.',
                          'Mixyard runtime/context/web exact-source binaries not exercised.',
                          'Private PipeWire isolation and synthetic PCM not exercised.',
                          'Routing, DSP, content capture, output, latency and endurance unqualified.',
                          'Windows and macOS native qualification unknown.'], 'artifacts': {}}
            try:
                value = capture(['/usr/bin/pw-dump'], Path('pw-dump.json'),
                                directory_fd=directory_fd, artifacts=report['artifacts'])
                report['objectCounts'] = inventory_summary(value)
                report['claims'].append('bounded native pw-dump JSON with unique u32 object IDs and one core')
                report['status'] = 'partial'
            except (OSError, ValueError, subprocess.TimeoutExpired) as error:
                report['status'] = 'blocked'
                report['failure'] = type(error).__name__
            # Exclusive creation also rejects an existing/symlinked report.
            with private_file('report.json', directory_fd) as output:
                output.write((json.dumps(report, indent=2) + '\n').encode())
            print(json.dumps(report, indent=2))
            return 0 if report['status'] == 'partial' else 2
    except CaptureCancelled:
        print('qualification cancelled after owned process cleanup')
        return 130
    except (OSError, ValueError, subprocess.TimeoutExpired):
        # Native stderr, paths and decoder input are deliberately not published.
        print('qualification blocked: host, evidence path or command unavailable')
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
