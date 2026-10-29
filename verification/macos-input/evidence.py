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


SOURCE_PATHS = ('adapters/macos/virtual-input', 'adapters/macos/input-broker',
                'verification/macos-input', '.github/workflows/macos-input.yml')


def capture(root, status):
    errors = []
    # Inspect actual objects, never local replacement history.
    git_env = dict(os.environ, GIT_NO_REPLACE_OBJECTS='1')

    def run_git(*args):
        return subprocess.run(['git', '-c', 'core.warnAmbiguousRefs=true', *args],
                              capture_output=True, env=git_env)

    def git(*args):
        result = run_git(*args)
        # rev-parse may succeed while warning that a short ref is ambiguous.
        if result.returncode or result.stderr:
            errors.append(f'git {args}: {os.fsdecode(result.stderr).strip()}')
            return None
        return result.stdout

    def text(*args):
        output = git(*args)
        return os.fsdecode(output).strip() if output is not None else ''

    def resolve(ref):
        return text('rev-parse', '--verify', '--end-of-options', ref + '^{commit}')

    def ancestor(older, newer):
        if not older or not newer:
            return None
        result = run_git('merge-base', '--is-ancestor', older, newer)
        if result.returncode not in (0, 1) or result.stderr:
            errors.append(f'git ancestry {older} {newer}: {os.fsdecode(result.stderr).strip()}')
            return None
        return result.returncode == 0

    def blob_hashes(revision):
        hashes = {}
        if not revision:
            return hashes
        listing = git('ls-tree', '-r', '-z', '--full-tree', revision, '--', *SOURCE_PATHS)
        for entry in (listing or b'').split(b'\0'):
            if not entry:
                continue
            metadata, path = entry.split(b'\t', 1)
            _, kind, oid = metadata.split()
            if kind != b'blob':
                errors.append(f'Scoped input must be a Git blob: {os.fsdecode(path)}')
                continue
            contents = git('cat-file', 'blob', oid.decode('ascii'))
            if contents is not None:
                hashes[os.fsdecode(path)] = hashlib.sha256(contents).hexdigest()
        return hashes

    def file_hashes(paths):
        hashes = {}
        for path in sorted(paths):
            try:
                hashes[str(path)] = hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()
            except OSError as error:
                errors.append(f'Cannot hash {path}: {error}')
        return hashes

    declared_ref = os.environ.get('MIXYARD_CURRENT_BASE', 'refs/remotes/origin/main')
    target_ref = os.environ.get('MIXYARD_TARGET_BASE_REF', 'refs/remotes/origin/main')
    executed = resolve('HEAD')
    source = resolve(os.environ.get('MIXYARD_SOURCE_HEAD', 'HEAD'))
    declared = resolve(declared_ref)
    # The workflow fetches this ref from the event's target branch independently
    # of HEAD and its parents. A commit ID or local/head ref cannot supply it.
    target = ''
    if (not target_ref.startswith('refs/remotes/origin/') or
            run_git('check-ref-format', target_ref).returncode):
        errors.append('Target base must be an explicit refs/remotes/origin/<base> ref.')
    else:
        target = resolve(target_ref)
    parents = text('show', '-s', '--format=%P', executed).split() if executed else []
    source_parents = text('show', '-s', '--format=%P', source).split() if source else []
    if text('rev-parse', '--is-shallow-repository') != 'false':
        errors.append('Complete ancestry required; shallow checkout cannot qualify.')
    if all((executed, source, declared, target)):
        # Traversal fails if a required ancestor object is missing, even if all
        # four tips resolve and an individual ancestry check could stop early.
        git('rev-list', '--quiet', executed, source, declared, target, '--')

    source_contains_declared = ancestor(declared, source)
    declared_approved = ancestor(declared, target)
    if declared_approved is not True:
        errors.append('Declared base must be an ancestor of the fetched target base.')
    merge_base = text('merge-base', source, declared) if source and declared else ''
    actual_base = parents[0] if executed != source and len(parents) >= 2 else None
    declared_in_executed_base = None
    executed_base_approved = None
    checkout_kind = 'invalid'
    if executed and source and executed == source:
        checkout_kind = 'source'
        if source_contains_declared is not True:
            errors.append('Direct source checkout must contain the declared base.')
    elif executed and source and len(parents) == 2 and parents[1] == source:
        checkout_kind = 'test_merge'
        declared_in_executed_base = ancestor(declared, actual_base)
        executed_base_approved = ancestor(actual_base, target)
        if declared_in_executed_base is not True:
            errors.append('Executed base must descend from the declared base.')
        if executed_base_approved is not True:
            errors.append('Executed base must be an ancestor of the fetched target base.')
    else:
        errors.append('Test merge must have exactly [executed base, source HEAD] parents.')

    source_hashes = blob_hashes(source)
    executed_hashes = blob_hashes(executed)
    working_hashes = file_hashes(executed_hashes)
    working_status = git('status', '--porcelain=v1', '-z', '--untracked-files=all', '--', *SOURCE_PATHS)
    if working_hashes != executed_hashes or working_status:
        errors.append('Scoped working inputs must match the executed commit.')
    artifacts = [p for p in pathlib.Path('verification/macos-input/build').rglob('*')
                 if p.is_file() and p.name != 'provenance.json']
    artifact_hashes = file_hashes(artifacts)
    source_tree = text('rev-parse', source + '^{tree}') if source else ''
    executed_tree = text('rev-parse', executed + '^{tree}') if executed else ''
    manifest = dict(schema_version=3, status=status, executed_head=executed,
        executed_parents=parents, source_head=source, source_parents=source_parents,
        # Compatibility: these still describe the declared base/source relation,
        # not the actual first parent of a possibly newer queued test merge.
        current_base=declared, merge_base=merge_base,
        declared_base_ref=declared_ref, declared_base=declared,
        actual_executed_base=actual_base, target_base_ref=target_ref, target_base=target,
        source_contains_declared_base=source_contains_declared,
        declared_base_ancestor_of_target=declared_approved,
        declared_base_ancestor_of_executed_base=declared_in_executed_base,
        executed_base_ancestor_of_target=executed_base_approved,
        checkout_kind=checkout_kind, source_tree=source_tree, executed_tree=executed_tree,
        foundation='d9c10984c18589ffa8efc54060bc4f1c6a8e2564',
        platform=platform.platform(), architecture=platform.machine(), provenance_errors=errors,
        provenance_valid=not errors,
        qualification_limit='CI artifact provenance only; a test merge includes approved base changes '
            'possibly absent from source. Direct source qualification covers only that source tree, '
            'not later target changes. No native authority, backend, or applied proof.',
        production_apply=False, system_bootstrap='BLOCKED/source_only',
        hosted_HAL_peer_identity='BLOCKED', external_consumer_pcm='BLOCKED', privacy_physical_quality='BLOCKED',
        source_sha256=source_hashes, executed_sha256=executed_hashes,
        working_tree_sha256=working_hashes, artifact_sha256=artifact_hashes)
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
