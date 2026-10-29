#!/usr/bin/env python3
"""Compile production sources against caller-specified read-only exact-lock dependencies.

No Cargo, downloads or shared target writes. Outputs are private and bounded;
this supplements, rather than replaces, hosted Cargo/three-OS qualification.
"""
import argparse
import os
import pathlib
import shutil
import subprocess
import tomllib

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--cache', type=pathlib.Path, required=True,
               help='existing cache root with network/Cargo.lock and target/debug/deps')
p.add_argument('--target', type=pathlib.Path, required=True, help='owned private /tmp output')
p.add_argument('--features', default='', help='comma-separated system-zstd,system-opus; empty is raw-only')
p.add_argument('--test', action='append', default=[])
p.add_argument('--lib-test-filter', help='compile production unit tests and run matching names')
p.add_argument('--clippy', action='store_true')
p.add_argument('--legacy', action='store_true', help='validate unchanged default network feature boundary')
a = p.parse_args()
root = pathlib.Path(__file__).resolve().parent
target = a.target.resolve()
assert target.is_relative_to('/tmp') and target != pathlib.Path('/tmp')
features = [s for s in a.features.split(',') if s]
assert set(features) <= {'system-zstd', 'system-opus'}
assert not (a.legacy and features)

def packages(path):
    return [p for p in tomllib.loads(path.read_text())['package']
            if p['name'] not in {'mixyard-network', 'mixyard-network-codecs'}]

assert packages(root / 'Cargo.lock') == packages(a.cache / 'network/Cargo.lock'), 'registry dependency lock mismatch'

def reserve():
    for path, minimum in [('/', 2 * 2**30), ('/tmp', 10 * 2**30), ('/dev/shm', 10 * 2**30)]:
        assert shutil.disk_usage(path).free >= minimum, f'{path} below reserve'
    generated = sum(f.stat().st_size for f in target.rglob('*') if f.is_file()) if target.exists() else 0
    assert generated <= 50 * 2**20, 'private output exceeds 50 MiB'

reserve()
target.mkdir(parents=True, exist_ok=True)
deps = a.cache / 'target/debug/deps'
base = ['clippy-driver' if a.clippy else 'rustc', '--edition=2024', '-C', 'debuginfo=0', '-D', 'warnings', '-L', f'dependency={deps}']
for directory in (a.cache / 'target/debug/build').glob('*/out'):
    base += ['-L', f'native={directory}']
for name in ['anyhow', 'quinn', 'rustls', 'serde', 'serde_json', 'tokio', 'rcgen']:
    paths = list(deps.glob(f'lib{name}-*.rlib'))
    assert len(paths) == 1, (name, paths)
    base += ['--extern', f'{name}={paths[0]}']
temporary = target / 'tmp'
temporary.mkdir(exist_ok=True)
env = dict(os.environ, CARGO_MANIFEST_DIR=str(root), CARGO_PKG_VERSION='0.1.0',
           CARGO_INCREMENTAL='0', TMPDIR=str(temporary))
flags = sum((['--cfg', f'feature="{s}"'] for s in features), [])
codec = target / 'libmixyard_network_codecs.rlib'
network = target / 'libmixyard_network.rlib'

def run(command):
    reserve()
    subprocess.run(command, check=True, env=env, timeout=180)
    reserve()

run(['rustc', '--edition=2024', '-C', 'debuginfo=0', '-D', 'warnings', *flags,
     '--crate-type=rlib', '--crate-name', 'mixyard_network_codecs',
     str(root / 'codecs/src/lib.rs'), '-o', str(codec)])
base += ['-L', f'dependency={target}']
if not a.legacy:
    base += ['--cfg', 'feature="codecs"', *flags, '--extern', f'mixyard_network_codecs={codec}']
run(base + ['--crate-type=rlib', '--crate-name', 'mixyard_network', str(root / 'src/lib.rs'), '-o', str(network)])
if a.lib_test_filter:
    binary = target / 'check'
    run(base + ['--crate-name', 'mixyard_network', '--test', str(root / 'src/lib.rs'), '-o', str(binary)])
    try:
        run([str(binary), a.lib_test_filter, '--test-threads=1', '--nocapture'])
    finally:
        binary.unlink(missing_ok=True)
default_tests = ['fragmentation', 'fragment_quic'] if a.legacy else ['codec_quic', 'fragmentation', 'fragment_quic']
for name in a.test or ([] if a.lib_test_filter else default_tests):
    binary = target / 'check'
    run(base + ['--extern', f'mixyard_network={network}', '--test', str(root / 'tests' / (name + '.rs')), '-o', str(binary)])
    try:
        run([str(binary), '--test-threads=1', '--nocapture'])
    finally:
        binary.unlink(missing_ok=True)
print('Production-source checks passed; hosted Cargo/Clippy and platform qualification remain separate.')
