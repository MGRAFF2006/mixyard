"""Host/SDK inventory and disk guard; neither implies native execution."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess


def disk_guard():
    free = shutil.disk_usage('.').free
    print(f'Available: {free / 2**30:.1f} GiB', flush=True)
    if free < 10 * 2**30:
        raise SystemExit('BLOCKED: less than 10 GiB free')


def command(args):
    try:
        result = subprocess.run(args, capture_output=True, text=True, timeout=60)
        return {'command': args, 'exit_code': result.returncode,
                'output': result.stdout + result.stderr}
    except (OSError, subprocess.TimeoutExpired) as error:
        return {'command': args, 'unavailable': str(error)}


def binary_info(binary):
    path = Path(binary)
    with path.open('rb') as file:
        digest = hashlib.file_digest(file, 'sha256').hexdigest()
    return {'path': str(path), 'bytes': path.stat().st_size, 'sha256': digest}


def inventory(binary=None):
    repo = Path(__file__).resolve().parents[2]
    desktop = repo / 'apps/desktop'
    sources = [*desktop.glob('src/**/*'), *desktop.glob('src-tauri/src/**/*'),
               *desktop.glob('src-tauri/capabilities/*'),
               *desktop.glob('src-tauri/permissions/*'),
               desktop / 'package.json', desktop / 'pnpm-lock.yaml',
               desktop / 'src-tauri/Cargo.toml', desktop / 'src-tauri/Cargo.lock',
               desktop / 'src-tauri/tauri.conf.json', desktop / 'src-tauri/build.rs',
               *desktop.glob('*.json'), *desktop.glob('*.ts'), desktop / 'index.html',
               repo / 'Cargo.toml', repo / 'Cargo.lock', *repo.glob('src/**/*'),
               *repo.glob('tests/runtime*.rs'),
               *repo.glob('verification/desktop-native/*.py'),
               *repo.glob('verification/desktop-native/*.sh'),
               repo / 'verification/desktop-native/Dockerfile',
               repo / 'verification/desktop-native/README.md',
               repo / '.github/workflows/desktop-native.yml',
               repo / '.github/workflows/runtime.yml']
    commands = [['rustc', '-vV'], ['cargo', '--version'], ['node', '--version'],
                ['pnpm', '--version'], ['cargo', 'install', '--list']]
    if platform.system() == 'Linux':
        commands += [['cat', '/etc/os-release'], ['ldd', '--version'],
                     ['dpkg-query', '-W'], ['pkg-config', '--modversion',
                      'webkit2gtk-4.1', 'gtk+-3.0', 'librsvg-2.0',
                      'ayatana-appindicator3-0.1']]
    elif platform.system() == 'Darwin':
        commands += [['sw_vers'], ['xcodebuild', '-version'],
                     ['xcrun', '--show-sdk-version'], ['clang', '--version']]
    elif platform.system() == 'Windows':
        commands += [['powershell', '-NoProfile', '-Command',
                      'Get-ComputerInfo | Select-Object WindowsProductName, '
                      'WindowsVersion, OsBuildNumber | Format-List; '
                      'Get-ItemProperty "HKLM:\\SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\*" '
                      '| Select-Object name,pv | Format-List; '
                      'Get-ChildItem "${env:ProgramFiles(x86)}\\Windows Kits\\10\\Include" '
                      '| Select-Object Name | Format-List'],
                     [str(Path(os.environ['ProgramFiles(x86)']) /
                          'Microsoft Visual Studio/Installer/vswhere.exe'),
                      '-latest', '-products', '*', '-format', 'json']]
        vswhere = str(Path(os.environ['ProgramFiles(x86)']) /
                      'Microsoft Visual Studio/Installer/vswhere.exe')
        installed = command([vswhere, '-latest', '-products', '*', '-property', 'installationPath'])
        if installed.get('exit_code') == 0 and installed['output'].strip():
            commands += [[str(path)] for path in sorted(
                Path(installed['output'].strip()).glob('VC/Tools/MSVC/*/bin/Hostx64/x64/cl.exe'))]
    report = {
        'schema_version': 1,
        'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'head': command(['git', 'rev-parse', 'HEAD']),
        'source_files': {str(path.relative_to(repo)):
                         hashlib.sha256(path.read_bytes()).hexdigest()
                         for path in sorted(set(sources)) if path.is_file()},
        'host': platform.platform(), 'machine': platform.machine(),
        'cpu_count': os.cpu_count(), 'load': os.getloadavg() if hasattr(os, 'getloadavg') else None,
        'runner': {key: os.environ.get(key) for key in
                   ['RUNNER_OS', 'RUNNER_ARCH', 'ImageOS', 'ImageVersion', 'GITHUB_RUN_ID']},
        'free_bytes': shutil.disk_usage('.').free,
        'commands': [command(args) for args in commands],
        'boundary': 'Inventory/build artifacts do not establish lifecycle, accessibility, timing or audio evidence.',
        'blocked': ['physical visible tray/menu and no-tray desktop hosts',
                    'Windows/macOS UI lifecycle, keyboard, accessibility and timing',
                    'screen-reader accessibility on every platform',
                    'audio engine survival across UI exit (no audio backend)',
                    'live meters, audio gaps, device latency and sound quality'],
    }
    if binary:
        report['binary'] = binary_info(binary)
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--guard', action='store_true')
    parser.add_argument('--output', type=Path)
    parser.add_argument('--binary')
    args = parser.parse_args()
    if args.guard:
        disk_guard()
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(inventory(args.binary), indent=2) + '\n')
