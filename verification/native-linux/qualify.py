#!/usr/bin/env python3
"""Read-only laptop discovery. Never launches an audio producer or mutates a graph."""
import argparse
import hashlib
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import resource
import subprocess
import tempfile

LIMIT = 8 * 1024 * 1024


def capture(argv, path):
    def bounds():
        resource.setrlimit(resource.RLIMIT_FSIZE, (LIMIT, LIMIT))
        os.umask(0o077)
    def private_file(target):
        return os.fdopen(os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), 'wb')
    with private_file(path) as output, private_file(path.with_suffix('.stderr')) as error:
        result = subprocess.run(argv, stdout=output, stderr=error, timeout=12,
                                preexec_fn=bounds, check=False)
    if result.returncode or path.stat().st_size >= LIMIT:
        raise ValueError('command failed or exceeded output bound')
    return json.loads(path.read_bytes())


def inventory_summary(value):
    if not isinstance(value, list) or not value:
        raise ValueError('expected nonempty pw-dump object array')
    ids = set()
    counts = {}
    for item in value:
        if not isinstance(item, dict) or type(item.get('id')) is not int:
            raise ValueError('invalid object identity')
        if item['id'] in ids:
            raise ValueError('duplicate object identity')
        ids.add(item['id'])
        kind = item.get('type')
        if not isinstance(kind, str) or not kind.startswith('PipeWire:Interface:'):
            raise ValueError('invalid object type')
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
    allowed = (root / 'verification/local-evidence').resolve()
    output = args.output.resolve()
    if allowed not in output.parents:
        parser.error('output must be beneath ignored verification/local-evidence')
    # Fail before native discovery unless the authorized host is established.
    with tempfile.TemporaryDirectory(prefix='mixyard-host-') as temp:
        host = capture(['tailscale', 'status', '--json'], Path(temp) / 'host.json')
        if host.get('Self', {}).get('HostName') != 'laptop':
            parser.error('not the authorized laptop')
    output.mkdir(parents=True, mode=0o700, exist_ok=False)
    report = {'schemaVersion': 1, 'label': 'native-read-only-metadata',
              'host': 'laptop', 'capturedAt': datetime.now(timezone.utc).isoformat(),
              'checkerSha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'sourceSha': subprocess.check_output(
                  ['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip(),
              'claims': [], 'gaps': [
                  'Snapshots do not establish complete inventory or stable selectors.',
                  'Mixyard runtime/context/web exact-source binaries not exercised.',
                  'Private PipeWire isolation and synthetic PCM not exercised.',
                  'Routing, DSP, content capture, output, latency and endurance unqualified.',
                  'Windows and macOS native qualification unknown.']}
    try:
        raw = output / 'pw-dump.json'
        value = capture(['/usr/bin/pw-dump'], raw)
        report['objectCounts'] = inventory_summary(value)
        report['claims'].append('bounded native pw-dump JSON with unique object IDs and one core')
        report['status'] = 'partial'
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        report['status'] = 'blocked'
        # Avoid serializing private stderr or decoder input into public summaries.
        report['failure'] = type(error).__name__
    report['artifacts'] = {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                           for p in output.iterdir() if p.is_file()}
    path = output / 'report.json'
    path.write_text(json.dumps(report, indent=2) + '\n')
    path.chmod(0o600)
    print(json.dumps(report, indent=2))
    return 0 if report['status'] == 'partial' else 2


if __name__ == '__main__':
    raise SystemExit(main())
