"""Compile and attest the exact native test executable; never infer it from a glob."""
import hashlib
import json
from pathlib import Path
import re
import subprocess


def compile_tests(manifest, target=None):
    manifest = Path(manifest).resolve()
    command = ['cargo', 'test', '--locked', '--no-run', '--message-format=json', '--manifest-path', str(manifest)]
    if target is not None:
        command += ['--target-dir', str(Path(target).resolve())]
    result = subprocess.run(command, capture_output=True, text=True, timeout=300)
    if result.returncode:
        codes = re.findall(r'error\[E[0-9]{4}\]', result.stdout + result.stderr)
        raise RuntimeError('Native test compilation failed: ' + json.dumps({'exit':result.returncode, 'compilerCodes':codes}))
    artifacts = []
    for line in result.stdout.splitlines():
        value = json.loads(line)
        if value.get('reason') != 'compiler-artifact' or not value.get('executable') or not value.get('profile', {}).get('test'):
            continue
        target_info = value.get('target', {})
        if target_info.get('name') != 'mixyard_desktop_control' or target_info.get('kind') != ['lib']:
            continue
        if Path(target_info.get('src_path', '')).resolve() != manifest.parent / 'src/lib.rs':
            raise RuntimeError('Native executable source does not match the requested crate')
        artifacts.append(Path(value['executable']).resolve())
    if len(artifacts) != 1:
        raise RuntimeError('Expected exactly one native library test executable')
    executable = artifacts[0]
    if target is not None and not executable.is_relative_to(Path(target).resolve()):
        raise RuntimeError('Historical executable escaped its isolated target')
    names = ['Cargo.toml', 'Cargo.lock'] + [str(path.relative_to(manifest.parent)).replace('\\','/') for path in sorted((manifest.parent/'src').glob('*.rs'))]
    sources = {name:hashlib.sha256((manifest.parent/name).read_bytes()).hexdigest() for name in names}
    return executable, {'sources':sources, 'executableSha256':hashlib.sha256(executable.read_bytes()).hexdigest()}
