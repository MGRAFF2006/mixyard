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
finish() {
  local result=$?
  trap - EXIT
  reserve || result=1
  python3 verification/macos-input/evidence.py "$evidence" "$result" || result=1
  exit "$result"
}
trap finish EXIT
python3 verification/macos-input/evidence.py "$evidence" incomplete
reserve
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
for needle in ('setCodeSigningRequirement','setConnectionCodeSigningRequirement','NSXPCConnectionPrivileged'):
    hits = [i for i, line in enumerate(lines) if needle in line]
    assert hits, f'SDK missing supported declaration: {needle}'
    for i in hits: print('\n'.join(lines[max(0,i-7):i+8]))
errors = (header.parent/'FoundationErrors.h').read_text().splitlines()
for needle in ('NSXPCConnectionCodeSigningRequirementFailure', 'NSXPCConnectionInvalid', 'NSXPCConnectionInterrupted'):
    hits = [i for i, line in enumerate(errors) if needle in line]
    assert hits, f'SDK missing documented error constant: {needle}'
    for i in hits: print('\n'.join(errors[max(0,i-3):i+4]))
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
import base64, json, pathlib, plistlib, re, struct, subprocess, sys, time
root = pathlib.Path(sys.argv[1]); app = root/'BrokerFixture.app'
(root/'authority-events').mkdir()
(root/'native-observations').mkdir()
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
    before = set((root/'authority-events').glob('*.json'))
    (root/f'{case}.inputs.json').write_text(json.dumps(dict(args=args, policy=policy), indent=2)+'\n')
    with (root/f'{case}.stdout').open('w') as out, (root/f'{case}.stderr').open('w') as err:
        try:
            result = subprocess.run([str(app/'Contents/MacOS/Producer'), *args], stdout=out, stderr=err, timeout=90)
        except subprocess.TimeoutExpired:
            (root/f'{case}.exit').write_text('timeout\n')
            raise
    (root/f'{case}.exit').write_text(str(result.returncode)+'\n')
    if result.returncode: raise SystemExit(f'FAIL: {case}; raw stdout/stderr/exit preserved in {root}')
    proof = json.loads((root/f'{case}.stdout').read_text())
    # Local receiver failures require 4102. A generic remote listener error is
    # only an observation until the live signed-peer/identical-request controls
    # (or same-signed-process policy A/B/A) and zero-authority evidence also pass.
    def matching(events, channel, pid, requirement, outcome):
        return [e for e in events if e['channel']==channel and e.get('peer_pid')==pid
                and e['requirement']==requirement and e['outcome']==outcome]
    def zero_state(state):
        assert all(state[k]==0 for k in ('active','generation','packets','hellos','cursor0','cursor1','start','end','revision'))
        assert state['credit_frames']==4096
    def remote_observation(error):
        assert error['domain']=='NSCocoaErrorDomain' and error['code'] in (4097,4099)
    deadline = time.monotonic()+5
    while True:
        events = [json.loads(p.read_text()) for p in sorted(set((root/'authority-events').glob('*.json'))-before)]
        if case == 'positive':
            stranger = proof['stranger']
            for key in ('error','late_hello_error','late_authentication_error'):
                remote_observation(stranger[key])
            assert stranger['terminal_invalidated'] is True and stranger['remote_rejection_observed'] is True
            controls = [proof['listener_control_before'], proof['listener_control_after']]
            assert controls[0]==controls[1]
            assert all(c['request_base64']==stranger['request_base64'] and c['early']=={'status':'not_admitted'}
                       and c['ready'] is True and c['broker_pid']==proof['broker']['pid'] for c in controls)
            assert base64.b64decode(stranger['request_base64'], validate=True)==struct.pack(
                '<IIIIQQQQIIff',1,48000,2,1,1,1,1,0,1,8,0.875,0.875)
            # The stranger's returned proof is itself received on a connection
            # pinned to its exact per-build native signature, after readiness.
            # Require its corresponding native actor session, not UID/PID identity.
            assert stranger['pid']!=proof['producer_pid'] and stranger['uid']==proof['broker']['uid']
            expected = [e for e in events if e['channel']=='pcm' and e['outcome']=='listener_pinned'
                        and e['broker_pid']==proof['broker']['pid'] and e['requirement']==policy['transport']]
            accepted = matching(events, 'control', proof['producer_pid'], policy['producer'], 'probe_reply')
            accepted = [e for e in accepted if e['broker_pid']==proof['broker']['pid']]
            # Held native probe replies must actually arrive after terminal close
            # on both channels; a source-only guard or an absent reply cannot pass.
            late = {e['channel'] for e in events if e['outcome']=='late_probe_ignored'
                    and e.get('broker_pid')==proof['broker']['pid'] and e['peer_pid']==proof['producer_pid']}
            if late != {'pcm', 'control'}: accepted = []
            controlled = matching(events, 'pcm', proof['producer_pid'], policy['transport'], 'session_closed')
            controlled = [e for e in controlled if e['broker_pid']==proof['broker']['pid']
                          and e['verified'] and e['ready'] and e['requests']==[1,1,1,0,1]]
            if len(controlled)!=2: accepted = []
            for event in controlled: zero_state(event['state'])
            actors = [e for e in events if e['channel']=='actor']
            checks = proof['actor_readiness_checks']
            assert len(checks)==12
            negative_ids = {c['connection'] for c in checks if 'connection' in c}
            for check in checks:
                if check['case']=='wrong_receiver':
                    assert check['error']['domain']=='NSCocoaErrorDomain'
                    assert check['error']['code']==proof['requirement_failure_code']
                    continue
                observed = [e for e in actors if e['connection']==check['connection']]
                assert all(e['endpoint_uses']==0 for e in observed)
                outcomes = {e['outcome'] for e in observed}
                if 'attempt_not_ready' not in outcomes: accepted = []
                if check['case']=='held_then_closed' and 'late_probe_ignored' not in outcomes: accepted = []
                if check['case']=='invalidated_unconfirmed' and not outcomes.intersection({'interrupted','invalidated'}):
                    accepted = []
            uses = [e for e in actors if e['outcome']=='endpoint_use']
            if len(uses)!=2: accepted = []  # Real stranger and trusted crash still execute.
            assert all(e['connection'] not in negative_ids and e['endpoint_uses']==1 for e in uses)
            assert all(any(r['connection']==e['connection'] and r['outcome']=='ready' for r in actors) for e in uses)
            assert all(e['peer_pid']==proof['producer_pid'] and e['requirement']==policy['producer'] for e in actors)
            assert all(e['endpoint_uses']==0 for e in actors if e['connection'] not in {u['connection'] for u in uses})
            stranger_uses = [e for e in uses if e['actor_pid']==stranger['pid']]
            if len(stranger_uses)!=1: accepted = []
            # The native listener rejects this peer before consulting its delegate.
            # There must be no PCM session/probe/admission for the stranger.
            assert not any(e['channel']=='pcm' and e.get('peer_pid')==stranger['pid'] for e in events)
            assert proof['authentication_before_authority'] is True
            assert proof['receiver_identity_reply_before_probe'] is True
            initial = proof['early_authority_unchanged']
            zero_state(initial)
            for key in ('stale_broker_error','stale_supervisor_broker_error','wrong_broker_error'):
                assert proof[key]['domain']=='NSCocoaErrorDomain'
                assert proof[key]['code']==proof['requirement_failure_code']
            assert proof['status']=='pass' and proof['external_consumer_pcm']=='BLOCKED'
        elif args[-1]=='inbound_control':
            assert proof['status']=='accepted_control'
            zero_state(proof['broker'])
            expected = matching(events, 'control', proof['producer_pid'], policy['producer'], 'probe_reply')
            expected = [e for e in expected if e['broker_pid']==proof['broker']['pid']]
            accepted = matching(events, 'control', proof['producer_pid'], policy['producer'], 'session_closed')
            accepted = [e for e in accepted if e['broker_pid']==proof['broker']['pid']
                        and e['verified'] and e['ready'] and e['requests']==[1,1,1,0,0]]
            for event in accepted: zero_state(event['state'])
        elif case=='stale-producer':
            assert proof['status']=='rejection_observed' and proof['case']==args[-1]
            remote_observation(proof['error'])
            expected = matching(events, 'control', proof['producer_pid'], policy['producer'], 'session_closed')
            expected = [e for e in expected if not e['verified'] and not e['ready'] and e['requests']==[0,0,0,0,0]]
            for event in expected: zero_state(event['state'])
            assert not matching(events, 'control', proof['producer_pid'], policy['producer'], 'probe_reply')
            accepted = [True]
        else:
            assert case in ('invalid-producer','missing-producer') and args[-1]=='policy_unavailable'
            assert proof['status']=='policy_unavailable_observed' and proof['case']=='policy_unavailable'
            remote_observation(proof['error'])  # Connection outcome, never live-identity proof.
            assert proof['terminal_invalidated'] is True and proof['receiver_verified'] is False
            expected = matching(events, 'control', proof['producer_pid'], policy.get('producer','<unavailable>'),
                                'policy_unavailable')
            for event in expected:
                assert event['terminal_rejection'] is True and event['session_created'] is False
                zero_state(event['state'])
                # The live broker initialized its valid anonymous transport, but
                # exported no control session, probe, readiness or authority.
                related = [e for e in events if e['broker_pid']==event['broker_pid']]
                assert all(e['outcome'] in ('listener_pinned','policy_unavailable') for e in related)
            expected = [e for e in expected if any(p['broker_pid']==e['broker_pid'] and p['channel']=='pcm'
                        and p['outcome']=='listener_pinned' and p['requirement']==policy['transport'] for p in events)]
            accepted = [True]
        if expected and accepted: break
        if time.monotonic() >= deadline:
            raise SystemExit(f'FAIL: {case}: no correlated authority outcome; raw events preserved')
        time.sleep(0.05)
    (root/f'{case}.authority.json').write_text(json.dumps(dict(
        status='pending_causal_controls' if case=='stale-producer' else 'pass',events=events),indent=2)+'\n')
    return proof
proof = run('positive', [policy['broker'],policy['stranger'],policy['crash'],'positive'])
accepted_policy = policy.copy()
# Causal A/B/A on fresh process-owned services, the SAME signed binaries and
# identical ready() RPCs. Only the expected inbound producer hash changes; the
# transport and outgoing broker pin stay valid. Controls never call hello.
control_args = [policy['broker'],policy['stranger'],policy['crash'],'inbound_control']
inbound_before = run('inbound-before', control_args)
policy['producer'] = 'cdhash H"0000000000000000000000000000000000000000"'
(root/'stale-authority.plist').write_bytes(plistlib.dumps(policy))
(root/'fixture-authority.plist').write_bytes(plistlib.dumps(policy))
stale = run('stale-producer', [policy['broker'],policy['stranger'],policy['crash'],'inbound_requirement_mismatch'])
stale_policy = policy.copy()
policy = accepted_policy.copy()
(root/'fixture-authority.plist').write_bytes(plistlib.dumps(policy))
inbound_after = run('inbound-after', control_args)
assert {key for key in policy if policy[key]!=stale_policy[key]}=={'producer'}
assert inbound_before['uid']==inbound_after['uid']==proof['broker']['uid']
assert len({inbound_before['producer_pid'],stale['producer_pid'],inbound_after['producer_pid']})==3
(root/'inbound-causal-controls.json').write_text(json.dumps(dict(status='pass',
    accepted_policy=policy, rejected_policy=stale_policy, before=inbound_before,
    rejection=stale, after=inbound_after, direct_enforcing_error=False,
    evidence_kind='same_signed_process_policy_ABA_and_zero_authority_lifetime'),indent=2)+'\n')
policy['producer'] = 'garbage'
# Keep the anonymous policy valid so unavailable supervisor policy is observed
# in the live listener delegate, rather than confusing process startup with auth.
policy['transport'] = plistlib.loads((root/'accepted-authority.plist').read_bytes())['transport']
(root/'invalid-authority.plist').write_bytes(plistlib.dumps(policy))
(root/'fixture-authority.plist').write_bytes(plistlib.dumps(policy))
run('invalid-producer', [policy['broker'],policy['stranger'],policy['crash'],'policy_unavailable'])
del policy['producer']
(root/'missing-authority.plist').write_bytes(plistlib.dumps(policy))
(root/'fixture-authority.plist').write_bytes(plistlib.dumps(policy))
run('missing-producer', [policy['broker'],policy['stranger'],policy['crash'],'policy_unavailable'])
PY
echo "PASS: native anonymous XPC fixture/auth/credit/lifetime; system/HAL/external PCM BLOCKED. Evidence: $evidence"
