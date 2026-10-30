#!/usr/bin/env python3
"""Owned hosted trust preparation with bounded, secret-free phase evidence.

No command runs outside a disposable GitHub macOS VM. Production Security PKI,
name validation, pinning and HTTP deadlines are unchanged.
"""
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time
from admin_fixture import OwnedAdmin


def phase(label, status):
    print(f'MIXYARD_PHASE {label}-{status}', file=sys.stderr, flush=True)


SECURITY_PHASES = frozenset(['startup-list', 'startup-create', 'startup-unlock', 'startup-search',
    'store-a', 'store-b', 'store-viewer', 'trust-a', 'trust-b', 'trust-viewer', 'cleanup-search', 'cleanup-verify',
    'cleanup-trust-a', 'cleanup-trust-b', 'cleanup-trust-viewer', 'cleanup-keychain',
    'admin-before', 'admin-ready', 'admin-precleanup', 'admin-after', 'cleanup-command'])


class StopIncomplete(RuntimeError):
    pass


# At most one sequential owned admin command. A timeout is not completion:
# retain its handle until sudo's monitor AND inherited output pipes are drained.
pending_command = None


def owned_process_evidence(label, child, budget):
    report = {'phase':label, 'state':'unavailable', 'processes':[]}
    try:
        if os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('RUNNER_OS') != 'macOS' or budget <= 0:
            raise RuntimeError('owned-host-required')
        if child.poll() is not None:
            report['state'] = 'owner-exited-pipes-unproved'
        else:
            result = subprocess.run(['ps','-axo','pid=,ppid=,uid=,stat=,comm='],check=True,capture_output=True,text=True,timeout=budget)
            if len(result.stdout)>1_048_576: raise RuntimeError('owned-process-bound')
            rows=[]
            for line in result.stdout.splitlines():
                fields=line.strip().split(None,4)
                if len(fields)!=5 or not all(value.isdecimal() for value in fields[:3]): continue
                name=Path(fields[4]).name
                if name not in ('sudo','security') or not re.fullmatch(r'[A-Za-z+<>-]{1,8}',fields[3]): continue
                rows.append({'pid':int(fields[0]),'parentPid':int(fields[1]),'uid':int(fields[2]),'state':fields[3],'comm':name})
            owned={child.pid}
            for _ in range(8):
                owned.update(row['pid'] for row in rows if row['parentPid'] in owned)
            report['processes']=[row for row in rows if row['pid'] in owned][:16]
            report['state']='captured'
    except (RuntimeError,OSError,subprocess.SubprocessError): pass
    print('MIXYARD_OWNED_PROCESS ' + json.dumps(report,separators=(',',':')),file=sys.stderr,flush=True)



def terminate_owned(child, label):
    # A successful syscall is NOT process/child/pipes completion. communicate()
    # still owns the only completion proof; errors retain pending_command.
    if label not in ('before-privileged-term', 'cleanup-privileged-term'):
        raise RuntimeError('owned-signal-phase-invalid')
    report = {'phase':label, 'pid':child.pid if type(child.pid) is int and 0 < child.pid < 2**31 else 0,
              'result':'accepted', 'errno':0}
    try:
        child.terminate()
    except OSError as error:
        report['result']='os-error'
        report['errno']=error.errno if type(error.errno) is int and 0 <= error.errno < 2**31 else 0
        print('MIXYARD_OWNED_SIGNAL ' + json.dumps(report,separators=(',',':')),file=sys.stderr,flush=True)
        raise
    print('MIXYARD_OWNED_SIGNAL ' + json.dumps(report,separators=(',',':')),file=sys.stderr,flush=True)



def command_with_default_term(command):
    # The cleanup parent ignores TERM to finish restoration. POSIX exec preserves
    # ignored dispositions; Popen.restore_signals does NOT reset SIGTERM. Reset
    # only in this fresh child interpreter, then exec SAME argv/PID. No shell,
    # preexec_fn/thread hazard, parent-signal window or elevated kill is involved.
    launcher = 'import os,signal,sys;signal.signal(signal.SIGTERM,signal.SIG_DFL);os.execvp(sys.argv[1],sys.argv[1:])'
    return [sys.executable, '-B', '-c', launcher, *command]



def completion_receipt(child, label):
    # Only call AFTER communicate() returned. Its wait()/EOF receipt is not a
    # census of detached privileged descendants and never proves trust removal.
    if type(child.returncode) is not int or not -2**31 < child.returncode < 2**31:
        raise StopIncomplete('owned-direct-child-reap-unproved')
    report = {'phase':label, 'pid':child.pid if type(child.pid) is int and 0 < child.pid < 2**31 else 0,
              'returncode':child.returncode, 'directChildReaped':True, 'pipesEof':True, 'descendants':'unproved'}
    print('MIXYARD_OWNED_RECEIPT '+json.dumps(report,separators=(',',':')),file=sys.stderr,flush=True)


def privileged_capture(command, timeout):
    global pending_command
    if os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('RUNNER_OS') != 'macOS':
        raise RuntimeError('owned-host-required')
    if pending_command is not None:
        raise StopIncomplete('owned-security-command-still-active')
    deadline = time.monotonic() + timeout
    child = subprocess.Popen(command_with_default_term(command), stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, text=True, start_new_session=True)
    pending_command = child
    reserve = min(0.5, timeout / 4)
    try:
        remaining = deadline-time.monotonic()-reserve
        if remaining <= 0: raise subprocess.TimeoutExpired('owned-security-start',timeout)
        output, errors = child.communicate(timeout=remaining)
    except subprocess.TimeoutExpired:
        # SIGTERM is relayed by sudo. SIGKILL of the monitor would not relay and
        # could leave a privileged Security child writing after our readback.
        try:
            owned_process_evidence('before-privileged-term',child,min(0.1,max(0,deadline-time.monotonic())))
            terminate_owned(child,'before-privileged-term')
            remaining=deadline-time.monotonic()
            if remaining<=0: raise subprocess.TimeoutExpired('owned-security-stop',timeout)
            child.communicate(timeout=min(reserve, remaining))
            completion_receipt(child,'privileged-timeout-reaped')
        except (OSError, subprocess.TimeoutExpired):
            raise StopIncomplete('owned-security-stop-incomplete') from None
        pending_command = None
        raise subprocess.TimeoutExpired('owned-security', timeout) from None
    pending_command = None
    if child.returncode:
        raise subprocess.CalledProcessError(child.returncode, 'owned-security')
    return subprocess.CompletedProcess('owned-security', 0, stdout=output, stderr=errors)


def drain_command(deadline):
    global pending_command
    child = pending_command
    if child is None: return True
    phase('cleanup-command', 'start')
    remaining = deadline - time.monotonic()
    try:
        if remaining <= 0: raise TimeoutError()
        owned_process_evidence('cleanup-privileged-term',child,min(0.2,remaining/4))
        terminate_owned(child,'cleanup-privileged-term')
        remaining=deadline-time.monotonic()
        if remaining<=0: raise TimeoutError()
        child.communicate(timeout=min(1, remaining))
        completion_receipt(child,'privileged-cleanup-reaped')
    except (OSError, TimeoutError, subprocess.TimeoutExpired, StopIncomplete):
        phase('cleanup-command', 'failed')
        return False  # Handle/state ownership remains; never report removal.
    pending_command = None
    phase('cleanup-command', 'done')
    return True


def run_owned(label, deadline, command, maximum=4):
    if label not in SECURITY_PHASES:
        raise RuntimeError('owned-security-phase-invalid')
    phase(label, 'start')
    remaining = deadline - time.monotonic()
    timeout = min(maximum, remaining)
    kind, value = None, 0
    try:
        if remaining <= 0:
            raise TimeoutError('owned-phase-budget')
        if command[:3] == ['sudo', '-n', '/usr/bin/security']:
            result = privileged_capture(command, timeout)
        else:
            result = subprocess.run(command, check=True, capture_output=True,
                                    text=True, timeout=timeout)
    except StopIncomplete:
        kind = 'stop-incomplete'
    except subprocess.TimeoutExpired:
        kind, value = 'timeout', max(0, round(timeout * 1000))
    except subprocess.CalledProcessError as error:
        code = error.returncode
        if type(code) is int and -2**31 < code < 2**31:
            kind, value = ('signal', -code) if code < 0 else ('exit', code)
        else:
            kind = 'subprocess-error'
    except TimeoutError:
        kind = 'budget-exhausted'
    except OSError as error:
        kind = 'os-error'
        value = error.errno if type(error.errno) is int and 0 <= error.errno < 2**31 else 0
    except subprocess.SubprocessError:
        kind = 'subprocess-error'
    if kind is not None:
        phase(label, 'failed')
        # Only our fixed phase/kind and an OS/process numeric classification.
        # NEVER stringify exceptions, argv, stdout, stderr or trust state.
        print('MIXYARD_SECURITY ' + json.dumps({'phase':label,'kind':kind,'value':value}, separators=(',', ':')), file=sys.stderr, flush=True)
        raise RuntimeError('owned-security-phase-failed') from None
    phase(label, 'done')
    return result.stdout


def security(label, deadline, *args, maximum=4):
    return run_owned(label, deadline, ['/usr/bin/security', *args], maximum=maximum)


def main():
    if os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('RUNNER_OS') != 'macOS':
        raise RuntimeError('owned-host-required')
    fixture = Path(os.environ['MIXYARD_NATIVE_FIXTURE'])
    keychain = fixture / 'qualification.keychain-db'
    if keychain.exists():
        raise RuntimeError('owned-keychain-already-exists')
    original = None
    admin = None
    created = False
    child = None
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt()
    signal.signal(signal.SIGTERM, interrupted)
    # Strictly inside the existing20s first-call total; no timeout increase.
    startup = time.monotonic() + 18
    try:
        admin = OwnedAdmin(fixture, run_owned)
        admin.admit(startup)
        original = re.findall(r'"([^"]+)"', security('startup-list', startup, 'list-keychains', '-d', 'user'))
        # Track ownership before commands that can partially complete and time out.
        created = True
        security('startup-create', startup, 'create-keychain', '-p', 'fixture-only', str(keychain))
        security('startup-unlock', startup, 'unlock-keychain', '-p', 'fixture-only', str(keychain))
        security('startup-search', startup, 'list-keychains', '-d', 'user', '-s', str(keychain), *original)
        for name in admin.names:
            admin.add(name, keychain, startup)
        admin.ready(startup)
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
        trust_clean = False
        stopped = drain_command(cleanup)
        if not stopped: failed = True
        if stopped and admin is not None:
            trust_clean = admin.cleanup(cleanup)
            if not trust_clean: failed = True
            # A timed-out REMOVE also owns a supervisor until actual drain.
            stopped = drain_command(cleanup)
            if not stopped: failed = True
        # Never race keychain deletion/admin removal with an unproved stop.
        if stopped and created and trust_clean:
            try: security('cleanup-keychain', cleanup, 'delete-keychain', str(keychain), maximum=2)
            except RuntimeError: failed = True
        elif created:
            phase('cleanup-recovery-retained','done')
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
