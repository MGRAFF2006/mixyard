#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
if [[ $(uname -s) != Darwin ]] || ! command -v xcrun >/dev/null; then
  echo 'BLOCKED: native macOS 13+ Foundation/Security/Apple ABI required.' >&2
  exit 2
fi
mkdir -p verification/macos-input/build
evidence=$(mktemp -d "verification/macos-input/build/broker-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-XXXXXX")
reserve() {
  python3 - "$evidence" <<'PY'
import shutil, sys
free = shutil.disk_usage(sys.argv[1]).free
print(f'free_bytes={free} reserve_bytes=10737418240')
if free < 10737418240: raise SystemExit('BLOCKED: exact 10 GiB reserve unavailable')
PY
}
reserve
trap reserve EXIT
sdk=$(xcrun --sdk macosx --show-sdk-path)
{
  sw_vers
  uname -m
  xcrun --sdk macosx --show-sdk-version
  xcrun --sdk macosx clang++ --version
  git rev-parse HEAD
  git show -s --format='%P' HEAD
} > "$evidence/toolchain.txt"
python3 - "$sdk" > "$evidence/selected-sdk-declarations.txt" <<'PY'
import pathlib, sys
header = pathlib.Path(sys.argv[1])/'System/Library/Frameworks/Foundation.framework/Headers/NSXPCConnection.h'
lines = header.read_text().splitlines()
for needle in ('setCodeSigningRequirement','NSXPCConnectionPrivileged'):
    hits = [i for i, line in enumerate(lines) if needle in line]
    assert hits, f'SDK missing supported declaration: {needle}'
    for i in hits: print('\n'.join(lines[max(0,i-7):i+8]))
PY
app="$evidence/BrokerFixture.app"
broker="$app/Contents/XPCServices/Broker.xpc"
stranger="$app/Contents/XPCServices/Stranger.xpc"
crash="$app/Contents/XPCServices/Crash.xpc"
python3 - "$app" <<'PY'
import pathlib, plistlib, sys
app = pathlib.Path(sys.argv[1])
for root, ident, name, kind in [(app, 'org.mixyard.producer.fixture', 'Producer', 'APPL'),
    (app/'Contents/XPCServices/Broker.xpc', 'org.mixyard.broker.fixture', 'Broker', 'XPC!'),
    (app/'Contents/XPCServices/Stranger.xpc', 'org.mixyard.stranger.fixture', 'Stranger', 'XPC!'),
    (app/'Contents/XPCServices/Crash.xpc', 'org.mixyard.crash.fixture', 'Crash', 'XPC!')]:
    (root/'Contents/MacOS').mkdir(parents=True)
    info = dict(CFBundleIdentifier=ident, CFBundleExecutable=name, CFBundlePackageType=kind,
        CFBundleVersion='1', CFBundleInfoDictionaryVersion='6.0', LSMinimumSystemVersion='13.0')
    if kind == 'XPC!': info['XPCService'] = dict(ServiceType='Application', RunLoopType='dispatch_main')
    (root/'Contents/Info.plist').write_bytes(plistlib.dumps(info))
PY
flags=(-std=c++17 -O2 -g -Wall -Wextra -Werror -fvisibility=hidden -fobjc-arc -fblocks
       -isysroot "$sdk" -mmacosx-version-min=13.0 -pthread)
frameworks=(-framework Foundation -framework Security -framework CoreAudio -framework CoreFoundation)
# Serial compilation, no shared native cache; jobs<=2. No consumer or HAL execution.
xcrun --sdk macosx clang++ "${flags[@]}" -DMIXYARD_INPUT_TESTING \
  adapters/macos/input-broker/broker.mm adapters/macos/virtual-input/input_driver.cpp \
  "${frameworks[@]}" -o "$broker/Contents/MacOS/Broker"
reserve
xcrun --sdk macosx clang++ "${flags[@]}" verification/macos-input/broker-fixture.mm \
  "${frameworks[@]}" -o "$app/Contents/MacOS/Producer"
xcrun --sdk macosx clang++ "${flags[@]}" -DMIXYARD_STRANGER verification/macos-input/broker-fixture.mm \
  "${frameworks[@]}" -o "$stranger/Contents/MacOS/Stranger"
cp "$app/Contents/MacOS/Producer" "$crash/Contents/MacOS/Crash"
# Compile the inert privileged system contract, never call or link it into HAL.
xcrun --sdk macosx clang++ "${flags[@]}" -c adapters/macos/input-broker/system_contract.mm \
  -o "$evidence/system-contract.o"
for bundle in "$broker" "$stranger" "$crash" "$app"; do
  codesign --force --sign - "$bundle"
  codesign --verify --strict --verbose=2 "$bundle"
  codesign --display --verbose=4 --requirements - "$bundle" >> "$evidence/signatures.txt" 2>&1
done
reserve
python3 - "$evidence" <<'PY'
import pathlib, plistlib, re, subprocess, sys
root = pathlib.Path(sys.argv[1]); app = root/'BrokerFixture.app'
def requirement(path):
    display = subprocess.check_output(['codesign','-d','--verbose=4',str(path)], stderr=subprocess.STDOUT, text=True)
    digest = re.search(r'^CDHash=([0-9a-f]{40})$', display, re.M).group(1)
    return f'cdhash H"{digest}"'
policy = {name: requirement(path) for name, path in {
    'producer':app, 'broker':app/'Contents/XPCServices/Broker.xpc',
    'stranger':app/'Contents/XPCServices/Stranger.xpc', 'crash':app/'Contents/XPCServices/Crash.xpc'}.items()}
assert policy['producer'] != policy['stranger']
policy['transport'] = policy['producer'] + ' or ' + policy['crash']
(root/'fixture-authority.plist').write_bytes(plistlib.dumps(policy))
(root/'accepted-authority.plist').write_bytes(plistlib.dumps(policy))
# Executions are bounded owned app/XPC subprocess fixtures. No launchctl, service
# installation/registration, HAL loading, TCC or real microphone access.
def run(case, args):
    with (root/f'{case}.stdout').open('w') as out, (root/f'{case}.stderr').open('w') as err:
        result = subprocess.run([str(app/'Contents/MacOS/Producer'), *args], stdout=out, stderr=err, timeout=90)
    (root/f'{case}.exit').write_text(str(result.returncode)+'\n')
    if result.returncode: raise SystemExit(f'FAIL: {case}; raw stdout/stderr/exit preserved in {root}')
run('positive', [policy['broker'],policy['stranger'],policy['crash'],'positive'])
# Fresh process-owned service instances; change only fixture policy, never host config.
policy['producer'] = 'cdhash H"0000000000000000000000000000000000000000"'
policy['transport'] = policy['producer']
(root/'stale-authority.plist').write_bytes(plistlib.dumps(policy))
(root/'fixture-authority.plist').write_bytes(plistlib.dumps(policy))
run('stale-producer', [policy['broker'],policy['stranger'],policy['crash'],'rejected'])
policy['producer'] = 'garbage'
policy['transport'] = 'garbage'
(root/'invalid-authority.plist').write_bytes(plistlib.dumps(policy))
(root/'fixture-authority.plist').write_bytes(plistlib.dumps(policy))
run('invalid-producer', [policy['broker'],policy['stranger'],policy['crash'],'rejected'])
PY
python3 - "$evidence" <<'PY'
import hashlib, json, pathlib, platform, subprocess, sys
root = pathlib.Path(sys.argv[1])
proof = json.loads((root/'positive.stdout').read_text())
assert proof['status']=='pass' and proof['external_consumer_pcm']=='BLOCKED'
for case in ('stale-producer','invalid-producer'):
    assert json.loads((root/f'{case}.stdout').read_text())['status']=='pass'
head = subprocess.check_output(['git','rev-parse','HEAD'], text=True).strip()
parents = subprocess.check_output(['git','show','-s','--format=%P','HEAD'], text=True).split()
files = [p for tree in ('adapters/macos/input-broker','adapters/macos/virtual-input','verification/macos-input')
         for p in pathlib.Path(tree).rglob('*') if p.is_file() and 'build' not in p.parts]
files += [pathlib.Path('.github/workflows/macos-input.yml')]
files += [p for p in root.rglob('*') if p.is_file()]
manifest = dict(schema_version=1, executed_head=head, executed_parents=parents,
    foundation='d9c10984c18589ffa8efc54060bc4f1c6a8e2564', platform=platform.platform(),
    anonymous_transport_authentication='executed_native_NSXPC_peer_code_requirements',
    peer_API='NSXPCConnection.setCodeSigningRequirement (macOS 13+)',
    policy='per-build exact cdhash fixture file; arbitrary same-user policy/memory tampering excluded',
    evidence_kind=proof['evidence_kind'], production_apply=False, release_driver='zeros_only_factory_export',
    system_bootstrap='BLOCKED/source_only', hosted_HAL_peer_identity='BLOCKED', external_consumer_pcm='BLOCKED',
    privacy_physical_quality='BLOCKED', sha256={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(files)})
(root/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
PY
echo "PASS: native anonymous XPC fixture/auth/credit/lifetime; system/HAL/external PCM BLOCKED. Evidence: $evidence"
