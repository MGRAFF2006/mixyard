#!/usr/bin/env bash
set -euo pipefail
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_OS:-}" == Linux ]] || { echo 'Hosted owned emulator required'; exit 1; }
[[ "${MIXYARD_HOSTED_EMULATOR_APPROVED:-}" == 1 ]] || { echo 'Explicit hosted emulator image approval required'; exit 1; }
[[ -r /dev/kvm && -w /dev/kvm ]] || { echo 'No owned hosted KVM qualification capacity'; exit 1; }
# Never attach to or replace another emulator. This disposable runner must have
# no connected device, and the selected console/ADB ports must be free.
export MIXYARD_OWNED_ADB="$ANDROID_HOME/platform-tools/adb"
[[ $("$MIXYARD_OWNED_ADB" devices | sed '1d;/^[[:space:]]*$/d' | wc -l) -eq 0 ]] || { echo 'Existing device prevents owned emulator qualification'; exit 1; }
python3 - <<'PORTS'
import socket
for port in (5554, 5555):
    with socket.socket() as listener: listener.bind(('127.0.0.1', port))
PORTS
owned_avd=$(mktemp -d "$RUNNER_TEMP/mixyard-confirm-avd.XXXXXX")
export ANDROID_AVD_HOME="$owned_avd"
image='system-images;android-35;default;x86_64'
"$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" "$image"
printf 'no\n' | "$ANDROID_HOME/cmdline-tools/latest/bin/avdmanager" create avd -n mixyard-confirmations -k "$image"
"$ANDROID_HOME/emulator/emulator" -avd mixyard-confirmations -port 5554 -no-window -no-audio -no-boot-anim -no-snapshot -gpu swiftshader_indirect -camera-back none -camera-front none > "$RUNNER_TEMP/owned-emulator.log" 2>&1 &
owned_emulator=$!
trap 'kill "$owned_emulator" 2>/dev/null || true' EXIT
export MIXYARD_OWNED_ADB="$ANDROID_HOME/platform-tools/adb"
timeout 180 bash -c '
  "$MIXYARD_OWNED_ADB" -s emulator-5554 wait-for-device
  until [[ $("$MIXYARD_OWNED_ADB" -s emulator-5554 shell getprop sys.boot_completed | tr -d "\r") == 1 ]]; do sleep 1; done
'
"$MIXYARD_OWNED_ADB" -s emulator-5554 install -r apps/mobile/android/app/build/outputs/apk/debug/app-debug.apk
"$MIXYARD_OWNED_ADB" -s emulator-5554 shell am instrument -w org.mixyard.mobile/org.mixyard.mobile.ConfirmationInstrumentation > "$RUNNER_TEMP/android-confirmations.txt"
python3 - <<'PYCODE'
import os
from pathlib import Path
text=(Path(os.environ['RUNNER_TEMP'])/'android-confirmations.txt').read_text()
assert 'mixyard-confirmations=passed' in text and 'INSTRUMENTATION_CODE: -1' in text, text
print('PASS owned Android native confirmation + actual shared UI fixture')
PYCODE
