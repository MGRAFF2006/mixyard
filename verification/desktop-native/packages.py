"""Require every declared installer format and fingerprint actual hosted outputs."""
import hashlib
import json
from pathlib import Path
import sys


def manifest(system, directory):
    formats = {'Linux': ['*.deb', '*.AppImage'], 'Windows': ['*.msi', '*-setup.exe'], 'macOS': ['*.dmg', '*.app/Contents/MacOS/mixyard-desktop']}[system]
    artifacts = []
    for pattern in formats:
        matches = sorted(directory.rglob(pattern))
        if not matches:
            raise SystemExit(f'Missing declared {system} installer artifact: {pattern}')
        for path in matches:
            with path.open('rb') as stream:
                digest = hashlib.file_digest(stream, 'sha256').hexdigest()
            artifacts.append({'path': str(path.relative_to(directory)), 'bytes': path.stat().st_size, 'sha256': digest})
    return {'platform': system, 'artifacts': artifacts, 'signing': 'disabled',
            'boundary': 'Generated hosted SDK bundles only; installation, upgrades, rollback, physical desktop, signing and notarization are unverified.'}


if __name__ == '__main__':
    system, directory, output = sys.argv[1:]
    Path(output).write_text(json.dumps(manifest(system, Path(directory)), indent=2) + '\n')
