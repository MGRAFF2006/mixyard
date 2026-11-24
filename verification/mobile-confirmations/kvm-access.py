"""Prepare only a verified existing KVM character node on an owned hosted VM."""
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess

NODE = Path('/dev/kvm')


def inspect_node():
    evidence = {'exists':False, 'characterDevice':False, 'runnerUid':os.getuid(), 'readable':False, 'writable':False}
    try:
        info = NODE.lstat()
    except FileNotFoundError:
        return evidence, None
    evidence.update(exists=True, characterDevice=stat.S_ISCHR(info.st_mode), ownerUid=info.st_uid,
                    ownerGid=info.st_gid, mode=stat.S_IMODE(info.st_mode), major=os.major(info.st_rdev), minor=os.minor(info.st_rdev))
    # lstat rejects symlinks. Match the kernel's actual registered misc device,
    # rather than trusting a generated filename or granting arbitrary nodes.
    try:
        registered = Path('/sys/class/misc/kvm/dev').read_text().strip()
    except OSError:
        registered = None
    evidence['kernelKvmMatch'] = evidence['characterDevice'] and registered == f"{evidence['major']}:{evidence['minor']}"
    evidence['readable'] = os.access(NODE, os.R_OK)
    evidence['writable'] = os.access(NODE, os.W_OK)
    return evidence, (info.st_dev, info.st_ino, info.st_rdev)


def emit(phase, evidence):
    print('Owned hosted KVM: ' + json.dumps({'phase':phase, **evidence}), flush=True)


def main():
    if os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('RUNNER_OS') != 'Linux' or os.environ.get('MIXYARD_HOSTED_EMULATOR_APPROVED') != '1':
        raise RuntimeError('Explicitly approved disposable hosted Linux VM required')
    before, identity = inspect_node()
    emit('before', before)
    if not before['exists'] or not before.get('kernelKvmMatch'):
        raise RuntimeError('No verified real hosted KVM character device')
    if before['readable'] and before['writable']:
        return
    uid = before['runnerUid']
    if uid <= 0:
        raise RuntimeError('Expected unprivileged hosted runner UID')
    acl = shutil.which('setfacl')
    owner = shutil.which('chown')
    sudo = shutil.which('sudo')
    if not sudo or not (acl or owner):
        raise RuntimeError('Existing noninteractive KVM permission tools unavailable')
    command = [sudo, '-n', acl, '-m', f'u:{uid}:rw', '--', str(NODE)] if acl else [sudo, '-n', owner, str(uid), '--', str(NODE)]
    try:
        result = subprocess.run(command, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5)
    except subprocess.TimeoutExpired:
        emit('grant-failed', {'kind':'timeout', 'value':5}); raise RuntimeError('Owned KVM grant timed out') from None
    except OSError as error:
        emit('grant-failed', {'kind':'os-error', 'value':error.errno}); raise RuntimeError('Owned KVM grant unavailable') from None
    if result.returncode:
        emit('grant-failed', {'kind':'signal' if result.returncode < 0 else 'exit', 'value':abs(result.returncode)})
        raise RuntimeError('Owned KVM grant denied')
    after, after_identity = inspect_node()
    emit('after', after)
    if after_identity != identity or not after.get('kernelKvmMatch') or not after['readable'] or not after['writable']:
        raise RuntimeError('Owned KVM node identity/access verification failed')


if __name__ == '__main__':
    main()
