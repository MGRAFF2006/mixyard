#!/usr/bin/env python3
"""Owned hosted trust preparation with bounded, secret-free phase evidence.

No command runs outside a disposable GitHub macOS VM. Production Security PKI,
name validation, pinning and HTTP deadlines are unchanged.
"""
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time


def phase(label, status):
    print(f'MIXYARD_PHASE {label}-{status}', file=sys.stderr, flush=True)


def security(label, deadline, *args, maximum=4):
    phase(label, 'start')
    remaining = deadline - time.monotonic()
    try:
        if remaining <= 0:
            raise TimeoutError('owned-phase-budget')
        result = subprocess.run(['security', *args], check=True, capture_output=True,
                                text=True, timeout=min(maximum, remaining))
    except (subprocess.SubprocessError, TimeoutError, OSError):
        phase(label, 'failed')
        raise RuntimeError('owned-security-phase-failed') from None
    phase(label, 'done')
    return result.stdout


def main():
    if os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('RUNNER_OS') != 'macOS':
        raise RuntimeError('owned-host-required')
    fixture = Path(os.environ['MIXYARD_NATIVE_FIXTURE'])
    keychain = fixture / 'qualification.keychain-db'
    if keychain.exists():
        raise RuntimeError('owned-keychain-already-exists')
    certificates = [(name, fixture / name / 'certificate.pem') for name in ('a', 'b', 'viewer')]
    original = None
    trusted = []
    created = False
    child = None
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt()
    signal.signal(signal.SIGTERM, interrupted)
    # Strictly inside the existing20s first-call total; no timeout increase.
    startup = time.monotonic() + 18
    try:
        original = re.findall(r'"([^"]+)"', security('startup-list', startup, 'list-keychains', '-d', 'user'))
        # Track ownership before commands that can partially complete and time out.
        created = True
        security('startup-create', startup, 'create-keychain', '-p', 'fixture-only', str(keychain))
        security('startup-unlock', startup, 'unlock-keychain', '-p', 'fixture-only', str(keychain))
        security('startup-search', startup, 'list-keychains', '-d', 'user', '-s', str(keychain), *original)
        for name, certificate in certificates:
            trusted.append((name, certificate))
            security('trust-' + name, startup, 'add-trusted-cert', '-r', 'trustRoot', '-k', str(keychain), str(certificate))
        executable = Path(__file__).parent / 'swift' / '.build' / 'native-control-qualification'
        phase('child-start', 'start')
        try: child = subprocess.Popen([str(executable)])
        except OSError:
            phase('child-start', 'failed'); raise RuntimeError('owned-native-spawn-failed') from None
        phase('child-start', 'done')
        code = child.wait()
        phase('child-exit', 'done' if code == 0 else 'failed')
        return code
    finally:
        # Parent shutdown remains bounded20s. Restore the original search list
        # FIRST, then remove only owned trust/keychain; each failure stays visible.
        # A caller timeout may send SIGTERM while preparation already unwinds.
        # Do not interrupt this bounded restoration; the parent's final reap
        # still bounds a genuinely stuck OS child.
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        cleanup = time.monotonic() + 14
        failed = False
        if child is not None and child.poll() is None:
            phase('cleanup-child', 'start')
            child.terminate()
            try:
                child.wait(timeout=2)
            except subprocess.TimeoutExpired:
                child.kill()
                try: child.wait(timeout=1)
                except subprocess.TimeoutExpired: failed = True
            phase('cleanup-child', 'failed' if failed else 'done')
        if original is not None:
            try: security('cleanup-search', cleanup, 'list-keychains', '-d', 'user', '-s', *original, maximum=3)
            except RuntimeError: failed = True
            try:
                restored = re.findall(r'"([^"]+)"', security('cleanup-verify', cleanup, 'list-keychains', '-d', 'user', maximum=2))
                if restored != original:
                    phase('cleanup-verify', 'failed'); failed = True
            except RuntimeError: failed = True
        for name, certificate in trusted:
            try: security('cleanup-trust-' + name, cleanup, 'remove-trusted-cert', str(certificate), maximum=2)
            except RuntimeError: failed = True
        if created:
            try: security('cleanup-keychain', cleanup, 'delete-keychain', str(keychain), maximum=2)
            except RuntimeError: failed = True
        phase('cleanup-complete', 'failed' if failed else 'done')
        if failed:
            raise RuntimeError('owned-trust-cleanup-failed')


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (Exception, KeyboardInterrupt):
        # No captured Security output, private paths, argv/password or state.
        print('Owned hosted Swift qualification failed; inspect fixed phase markers', file=sys.stderr)
        sys.exit(1)
