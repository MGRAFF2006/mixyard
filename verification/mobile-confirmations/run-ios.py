"""Run only on a disposable hosted Mac; create/delete one owned simulator."""
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import subprocess
import time
from architecture import remaining, verify_artifacts
from owned_simulator_state import require_booted_simulator, capture_runtime_catalogue


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


def capture(label, *args, timeout=30):
    print('Owned simulator capture: ' + label + '-start', flush=True)
    try:
        value = subprocess.check_output(args, text=True, timeout=timeout, stdin=subprocess.DEVNULL, stderr=subprocess.PIPE).strip()
    except subprocess.TimeoutExpired:
        print('Owned simulator capture failure: ' + json.dumps({'phase':label, 'kind':'timeout', 'value':timeout}), flush=True)
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



def toolchain_evidence():
    # Read-only evidence. No first-launch setup/install, service restart, retry,
    # user GUI interaction or change to the runtime-discovery30s deadline.
    step('first-launch-status', ['xcodebuild', '-checkFirstLaunchStatus'], 30)
    simctl = Path(capture('resolved-simctl', 'xcrun', '--find', 'simctl')).resolve()
    developer = Path(os.environ.get('DEVELOPER_DIR') or capture('selected-developer', 'xcode-select', '-p')).resolve()
    expected = developer / 'usr/bin/simctl'
    candidates = [developer / 'Library/PrivateFrameworks/CoreSimulator.framework/Resources/Info.plist',
                  Path('/Library/Developer/PrivateFrameworks/CoreSimulator.framework/Resources/Info.plist')]
    versions = []
    for location in candidates:
        try:
            value = plistlib.loads(location.read_bytes())
        except (OSError, plistlib.InvalidFileException):
            continue
        fields = [value.get('CFBundleShortVersionString'), value.get('CFBundleVersion')]
        if all(isinstance(field, str) and re.fullmatch(r'[0-9]+(?:\.[0-9]+)*', field) for field in fields):
            versions.append({'scope':'selected-xcode' if location == candidates[0] else 'shared-developer',
                             'version':fields[0], 'build':fields[1]})
    print('Owned simulator tool identity: ' + json.dumps({'simctlMatchesSelectedXcode':simctl == expected.resolve(),
          'coreSimulatorVersions':versions}), flush=True)
    if simctl != expected.resolve():
        raise RuntimeError('Resolved simctl does not belong to selected Xcode')



def validate_comparison(xcode, sdk):
    lane = os.environ.get('MIXYARD_CONFIRMATION_COMPARISON')
    if lane is None: return
    expected = '/Applications/Xcode_16.2.app/Contents/Developer'
    if lane != 'xcode16.2-sdk18.2' or os.environ.get('DEVELOPER_DIR') != expected or not Path(expected).is_dir():
        raise RuntimeError('COMPARISON requires the exact preinstalled Xcode16.2; no install/fallback')
    if xcode.splitlines()[0] != 'Xcode 16.2' or version(sdk)[:2] != (18,2):
        raise RuntimeError('COMPARISON selected toolchain/SDK mismatch; no fallback')
    print('Owned simulator COMPARISON: preinstalled Xcode16.2 / SDK18.2; same assertions/budgets, not causal repair', flush=True)



def destination_evidence(text, identifier):
    records=[];availability='unknown'
    for line in text.splitlines():
        if 'Available destinations for' in line: availability='available'
        if 'Ineligible destinations for' in line: availability='ineligible'
        for entry in re.findall(r'\{([^{}]+)\}',line):
            pairs=[part.strip().split(':',1) for part in entry.split(',') if ':' in part]
            fields={key.strip():value.strip() for key,value in pairs}
            if fields.get('id') != identifier: continue
            records.append({'platform':fields.get('platform') if fields.get('platform') in ('iOS Simulator','iOS','macOS') else 'other',
                            'arch':fields.get('arch') if fields.get('arch') in ('arm64','x86_64') else 'missing-or-other',
                            'archPresent':'arch' in fields,'errorPresent':'error' in fields,'availability':availability,
                            'duplicateFields':len(fields)!=len(pairs)})
    return {'ownedUuidMentioned':identifier in text,'records':records[:8],'recordLimitExceeded':len(records)>8}


def destination_structure(text, identifier):
    """Non-authorizing bounded witness; never used by destination admission."""
    section='unknown';sections={'available':False,'ineligible':False}
    braces={'available':0,'ineligible':0,'unknown':0}
    command_mention=False;section_mention=False;outside_mention=False
    exact=0;owned_lines=0;candidates=[]
    known={'id','platform','arch','name','error','OS','variant'}
    for index,line in enumerate(text.splitlines(),1):
        if 'Available destinations for' in line:
            section='available';sections[section]=True
        if 'Ineligible destinations for' in line:
            section='ineligible';sections[section]=True
        mentioned=identifier in line
        command_like='xcodebuild' in line and '-showdestinations' in line
        entries=re.findall(r'\{([^{}]+)\}',line)
        owned_records=[];owned_braces=0
        for entry in entries:
            braces[section]+=1
            pairs=[part.strip().split(':',1) for part in entry.split(',') if ':' in part]
            fields={key.strip():value.strip() for key,value in pairs}
            if fields.get('id')==identifier:exact+=1
            if identifier in entry:
                owned_braces+=1
                if len(owned_records)<4:
                    owned_records.append({'fieldCount':min(len(pairs),16),'fieldCountCapped':len(pairs)>16,
                        'knownKeyCount':min(sum(key.strip() in known for key,_ in pairs),16),
                        'idPresent':'id' in fields,'exactIdEqual':fields.get('id')==identifier,
                        'platform':fields.get('platform') if fields.get('platform') in ('iOS Simulator','iOS','macOS') else 'other',
                        'arch':fields.get('arch') if fields.get('arch') in ('arm64','x86_64') else 'missing-or-other',
                        'archPresent':'arch' in fields,'errorPresent':'error' in fields,
                        'duplicateFields':len(fields)!=len(pairs)})
        if not mentioned:continue
        owned_lines+=1
        command_mention |= command_like
        section_mention |= section!='unknown'
        outside_mention |= section=='unknown'
        if len(candidates)<8:
            opened=line.count('{');closed=line.count('}')
            shape='none' if opened==closed==0 else 'balanced' if opened==closed else 'open-only' if not closed else 'close-only' if not opened else 'unbalanced'
            candidates.append({'lineIndex':min(index,65535),'lineIndexCapped':index>65535,
                'section':section,'commandEchoLike':command_like,'braceShape':shape,
                'braceRecords':min(len(entries),8),'braceRecordLimitExceeded':len(entries)>8,
                'keysOnLine':{key:bool(re.search(r'\b'+key+r'\s*:',line)) for key in ('id','platform','arch','error')},
                'ownedBraceRecords':min(owned_braces,4),'ownedBraceRecordLimitExceeded':owned_braces>4,
                'ownedRecords':owned_records})
    return {'availableSection':sections['available'],'ineligibleSection':sections['ineligible'],
            'uuidInCommandEchoLike':command_mention,'uuidInDestinationSections':section_mention,
            'uuidOutsideDestinationSections':outside_mention,
            'braceRecordsBySection':{key:min(value,64) for key,value in braces.items()},
            'braceCountLimitExceeded':any(value>64 for value in braces.values()),
            'exactOwnedIdMatches':min(exact,64),'exactMatchLimitExceeded':exact>64,
            'ownedBearingLines':min(owned_lines,64),'candidateLimitExceeded':owned_lines>8,
            'candidates':candidates}


def shutdown_owned(identifier):
    deadline=time.monotonic()+30
    try:
        step('cleanup-shutdown',['xcrun','simctl','shutdown',identifier],30)
    except subprocess.CalledProcessError as error:
        if error.returncode != 149: raise
        remaining=deadline-time.monotonic()
        if remaining<=0: raise
        # Exit149 alone is NEVER success. Verify the unique UUID we created is
        # actually Shutdown, in the remainder of the SAME30s cleanup budget.
        raw=capture('cleanup-owned-state','xcrun','simctl','list','devices','--json',timeout=min(3,remaining))
        value=json.loads(raw)
        entries=[device for group in value['devices'].values() for device in group if device.get('udid')==identifier]
        if len(entries)!=1 or entries[0].get('state')!='Shutdown': raise error
        print('Owned simulator phase: cleanup-shutdown-postcondition-verified',flush=True)


def main():
    if os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('RUNNER_OS') != 'macOS':
        raise RuntimeError('Disposable hosted Mac required')
    step('locked-trust-package', ['python3', '-B', 'verification/mobile-native/swift-package-lock.py', '--xcode'], 30)
    xcode = capture('selected-xcode', 'xcodebuild', '-version')
    sdk = capture('selected-sdk', 'xcrun', '--sdk', 'iphonesimulator', '--show-sdk-version')
    validate_comparison(xcode,sdk)
    arch = platform.machine()
    print('Owned simulator toolchain: ' + json.dumps({'xcode':xcode, 'sdk':sdk, 'arch':arch}), flush=True)
    toolchain_evidence()
    process_evidence('before-runtime-discovery')
    runtimes = json.loads(capture_runtime_catalogue())['runtimes']
    runtime = select_runtime(sdk, runtimes, arch)
    print('Owned simulator selection: ' + json.dumps({'xcode':xcode, 'sdk':sdk, 'runtime':runtime['version'], 'arch':arch}), flush=True)
    device_type = 'com.apple.CoreSimulator.SimDeviceType.iPhone-16'
    identifier = capture('owned-create', 'xcrun', 'simctl', 'create', 'mixyard-confirmations',
                         device_type, runtime['identifier'])
    if not re.fullmatch(r'[0-9A-Fa-f-]{36}', identifier):
        raise RuntimeError('Owned simulator returned invalid identifier')
    first_failure = None
    cleanup_failures = []
    try:
        output = Path(os.environ['RUNNER_TEMP']) / 'mixyard-confirm-ios'
        destination = f'platform=iOS Simulator,id={identifier}'
        common = ['-project', 'apps/mobile/ios/Mixyard.xcodeproj', '-scheme', 'MixyardConfirmations',
                  '-configuration', 'Debug', '-sdk', 'iphonesimulator', '-destination', destination,
                  '-onlyUsePackageVersionsFromResolvedFile', '-disableAutomaticPackageResolution',
                  '-derivedDataPath', str(output), 'ONLY_ACTIVE_ARCH=YES', f'ARCHS={arch}', 'SWIFT_ACTIVE_COMPILATION_CONDITIONS=DEBUG OWNED_CONFIRMATION_EVIDENCE', 'CODE_SIGNING_ALLOWED=NO']
        step('locked-package-resolution', ['xcodebuild', '-resolvePackageDependencies', *common], 120)
        # Explicit prepared-device contract, not a discovery-cause claim.
        step('boot', ['xcrun', 'simctl', 'boot', identifier], 30)
        step('bootstatus', ['xcrun', 'simctl', 'bootstatus', identifier, '-b'], 180)
        require_booted_simulator(identifier,runtime['identifier'],device_type)
        # One actual supported simulator architecture for app, local package and
        # UI-test target. No EXCLUDED_ARCHS, suppressed import or skipped test.
        budget=time.monotonic()+900
        step('actual-build-for-testing',['xcodebuild','build-for-testing',*common],remaining(budget,900))
        verify_artifacts(output,arch,budget,capture)
        step('actual-shared-trust-link-proof', ['python3', '-B', 'verification/mobile-native/xcode-trust-evidence.py', str(output), arch], remaining(budget,120))
        step('actual-ui-test',['xcodebuild','test-without-building',*common],remaining(budget,900))
    except BaseException as error:
        first_failure = error
    finally:
        # Both independent owned cleanup operations are attempted. Never mask
        # the FIRST boot/build/test error with a later cleanup deadline.
        for operation in ('shutdown', 'delete'):
            try:
                if operation=='shutdown': shutdown_owned(identifier)
                else: step('cleanup-' + operation, ['xcrun', 'simctl', operation, identifier], 30)
            except Exception as error: cleanup_failures.append(error)
    if first_failure is not None:
        if cleanup_failures:
            raise first_failure from ExceptionGroup('Owned simulator cleanup also failed', cleanup_failures)
        raise first_failure
    if cleanup_failures:
        raise ExceptionGroup('Owned simulator cleanup failed', cleanup_failures)


if __name__ == '__main__':
    main()
