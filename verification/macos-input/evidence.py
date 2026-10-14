#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Bind successful or partial native artifacts to explicit, complete git ancestry."""
import argparse
import hashlib
import json
import os
import pathlib
import platform
import subprocess


def capture(root, status):
    errors = []

    def git(*args):
        result = subprocess.run(['git', *args], text=True, capture_output=True)
        if result.returncode:
            errors.append(f'git {args}: {result.stderr.strip()}')
        return result.stdout.strip()

    executed = git('rev-parse', '--verify', 'HEAD^{commit}')
    source = git('rev-parse', '--verify', os.environ.get('MIXYARD_SOURCE_HEAD', 'HEAD') + '^{commit}')
    base = git('rev-parse', '--verify', os.environ.get('MIXYARD_CURRENT_BASE', 'origin/main') + '^{commit}')
    parents = git('show', '-s', '--format=%P', 'HEAD').split()
    source_parents = git('show', '-s', '--format=%P', source).split() if source else []
    merge_base = git('merge-base', source, base) if source and base else ''
    if git('rev-parse', '--is-shallow-repository') != 'false':
        errors.append('Complete ancestry required; shallow checkout cannot qualify.')
    if not parents or not source_parents:
        errors.append('Missing executed/source parents.')
    if not base or merge_base != base:
        errors.append('Current base must be integrated into source HEAD.')
    if executed != source and parents != [base, source]:
        errors.append('Executed commit must be source HEAD or exact [current base, source HEAD] test merge.')
    sources = git('ls-files', '-z', '--', 'adapters/macos/virtual-input',
                  'adapters/macos/input-broker', 'verification/macos-input', '.github/workflows/macos-input.yml')
    source_files = [pathlib.Path(p) for p in sources.split('\0') if p]
    artifacts = [p for p in pathlib.Path('verification/macos-input/build').rglob('*')
                 if p.is_file() and p.name != 'provenance.json']
    manifest = dict(schema_version=2, status=status, executed_head=executed,
        executed_parents=parents, source_head=source, source_parents=source_parents,
        current_base=base, merge_base=merge_base,
        source_tree=git('rev-parse', source + '^{tree}') if source else '',
        executed_tree=git('rev-parse', 'HEAD^{tree}'),
        foundation='d9c10984c18589ffa8efc54060bc4f1c6a8e2564',
        platform=platform.platform(), architecture=platform.machine(), provenance_errors=errors,
        production_apply=False, system_bootstrap='BLOCKED/source_only',
        hosted_HAL_peer_identity='BLOCKED', external_consumer_pcm='BLOCKED', privacy_physical_quality='BLOCKED',
        source_sha256={str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(source_files)},
        artifact_sha256={str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(artifacts)})
    root.mkdir(parents=True, exist_ok=True)
    (root/'provenance.json').write_text(json.dumps(manifest, indent=2) + '\n')
    if errors:
        raise SystemExit('\n'.join(errors))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=pathlib.Path)
    parser.add_argument('status', help='incomplete, failure, success, or script exit code; never inferred from files')
    args = parser.parse_args()
    capture(args.directory, args.status)
