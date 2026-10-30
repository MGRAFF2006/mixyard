"""Run only on a disposable hosted Mac; create/delete one owned simulator."""
import json
import os
from pathlib import Path
import platform
import re
import subprocess


def version(value):
    if not isinstance(value, str) or not re.fullmatch(r"[0-9]+(?:\.[0-9]+){1,2}", value):
        raise RuntimeError('Invalid SDK/runtime version')
    return tuple(int(part) for part in value.split('.'))


def select_runtime(sdk, runtimes, arch):
    if arch not in ('arm64', 'x86_64'):
        raise RuntimeError('Unsupported native simulator architecture')
    sdk_version = version(sdk)
    matches = [value for value in runtimes if value.get('isAvailable') is True and
               value.get('identifier', '').startswith('com.apple.CoreSimulator.SimRuntime.iOS-') and
               version(value.get('version'))[:2] == sdk_version[:2]]
    if not matches:
        # Do not choose a newer arbitrary runtime, download one or silently skip.
        raise RuntimeError('No installed iOS runtime matches the selected SDK major/minor')
    return max(matches, key=lambda value: version(value['version']))


def process_evidence(label):
    # Read-only names/IDs only, never argv, environment, files or unrelated apps.
    allowed = {'com.apple.CoreSimulator.CoreSimulatorService', 'CoreSimulatorService', 'Simulator', 'simctl', 'xcrun', 'xcodebuild'}
    try:
        result = subprocess.run(['ps', '-axo', 'pid=,ppid=,comm='], check=True, capture_output=True, text=True, timeout=3)
        rows = []
        for line in result.stdout.splitlines():
            fields = line.strip().split(None, 2)
            if len(fields) != 3 or not fields[0].isdigit() or not fields[1].isdigit(): continue
            name = Path(fields[2]).name
            if name in allowed and len(rows) < 32: rows.append({'pid':int(fields[0]), 'parentPid':int(fields[1]), 'name':name})
        print('Owned simulator process evidence: ' + json.dumps({'phase':label, 'processes':rows}), flush=True)
    except (OSError, subprocess.SubprocessError):
        print('Owned simulator process evidence unavailable: ' + label, flush=True)


def capture(label, *args):
    print('Owned simulator capture: ' + label + '-start', flush=True)
    try:
        value = subprocess.check_output(args, text=True, timeout=30, stdin=subprocess.DEVNULL).strip()
    except subprocess.TimeoutExpired:
        print('Owned simulator capture failure: ' + json.dumps({'phase':label, 'kind':'timeout', 'value':30}), flush=True)
        process_evidence(label + '-failed'); raise
    except subprocess.CalledProcessError as error:
        code = error.returncode
        print('Owned simulator capture failure: ' + json.dumps({'phase':label, 'kind':'signal' if code < 0 else 'exit', 'value':abs(code)}), flush=True)
        process_evidence(label + '-failed'); raise
    except OSError as error:
        print('Owned simulator capture failure: ' + json.dumps({'phase':label, 'kind':'os-error', 'value':error.errno}), flush=True)
        process_evidence(label + '-failed'); raise
    print('Owned simulator capture: ' + label + '-done', flush=True)
    return value


def step(label, args, timeout):
    print('Owned simulator phase: ' + label + '-start', flush=True)
    try:
        result = subprocess.run(args, check=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        print('Owned simulator failure: ' + json.dumps({'phase':label, 'kind':'timeout', 'value':timeout}), flush=True)
        raise
    except subprocess.CalledProcessError as error:
        code = error.returncode
        print('Owned simulator failure: ' + json.dumps({'phase':label, 'kind':'signal' if code < 0 else 'exit', 'value':abs(code)}), flush=True)
        raise
    except OSError as error:
        print('Owned simulator failure: ' + json.dumps({'phase':label, 'kind':'os-error', 'value':error.errno}), flush=True)
        raise
    print('Owned simulator phase: ' + label + '-done', flush=True)
    return result


def main():
    if os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('RUNNER_OS') != 'macOS':
        raise RuntimeError('Disposable hosted Mac required')
    xcode = capture('selected-xcode', 'xcodebuild', '-version')
    sdk = capture('selected-sdk', 'xcrun', '--sdk', 'iphonesimulator', '--show-sdk-version')
    arch = platform.machine()
    print('Owned simulator toolchain: ' + json.dumps({'xcode':xcode, 'sdk':sdk, 'arch':arch}), flush=True)
    process_evidence('before-runtime-discovery')
    runtimes = json.loads(capture('runtime-discovery', 'xcrun', 'simctl', 'list', 'runtimes', '--json'))['runtimes']
    runtime = select_runtime(sdk, runtimes, arch)
    print('Owned simulator selection: ' + json.dumps({'xcode':xcode, 'sdk':sdk, 'runtime':runtime['version'], 'arch':arch}), flush=True)
    identifier = capture('owned-create', 'xcrun', 'simctl', 'create', 'mixyard-confirmations',
                         'com.apple.CoreSimulator.SimDeviceType.iPhone-16', runtime['identifier'])
    if not re.fullmatch(r'[0-9A-Fa-f-]{36}', identifier):
        raise RuntimeError('Owned simulator returned invalid identifier')
    first_failure = None
    cleanup_failures = []
    try:
        output = Path(os.environ['RUNNER_TEMP']) / 'mixyard-confirm-ios'
        destination = f'platform=iOS Simulator,id={identifier},arch={arch}'
        common = ['-project', 'apps/mobile/ios/Mixyard.xcodeproj', '-scheme', 'MixyardConfirmations',
                  '-configuration', 'Debug', '-sdk', 'iphonesimulator', '-destination', destination,
                  '-derivedDataPath', str(output), 'ONLY_ACTIVE_ARCH=YES', f'ARCHS={arch}', 'CODE_SIGNING_ALLOWED=NO']
        destinations = capture('owned-destinations', 'xcodebuild', '-showdestinations', *common)
        if not any(identifier in line and re.search(r'\barch:\s*' + re.escape(arch) + r'\b', line) for line in destinations.splitlines()):
            raise RuntimeError('Owned simulator destination does not advertise the native architecture')
        step('boot', ['xcrun', 'simctl', 'boot', identifier], 30)
        step('bootstatus', ['xcrun', 'simctl', 'bootstatus', identifier, '-b'], 180)
        # One actual supported simulator architecture for app, local package and
        # UI-test target. No EXCLUDED_ARCHS, suppressed import or skipped test.
        step('actual-ui-test', ['xcodebuild', 'test', *common], 900)
    except BaseException as error:
        first_failure = error
    finally:
        # Both independent owned cleanup operations are attempted. Never mask
        # the FIRST boot/build/test error with a later cleanup deadline.
        for operation in ('shutdown', 'delete'):
            try: step('cleanup-' + operation, ['xcrun', 'simctl', operation, identifier], 30)
            except Exception as error: cleanup_failures.append(error)
    if first_failure is not None:
        if cleanup_failures:
            raise first_failure from ExceptionGroup('Owned simulator cleanup also failed', cleanup_failures)
        raise first_failure
    if cleanup_failures:
        raise ExceptionGroup('Owned simulator cleanup failed', cleanup_failures)


if __name__ == '__main__':
    main()
