#!/usr/bin/env bash
set -euo pipefail
[[ "${GITHUB_ACTIONS:-}" == true ]] || { echo 'This build qualification requires an owned hosted runner' >&2; exit 1; }
[[ $# == 3 ]] || { echo 'Usage: run-jvm.sh ENGINE_CHECKOUT ACTUAL_ENGINE_LIBRARY OWNED_FIXTURE_JSON' >&2; exit 1; }
task_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
task_build="$RUNNER_TEMP/mixyard-mobile-host-jni"
mkdir -p "$task_root/verification/mobile-host/kotlin/build/production"
cp "$task_root/apps/mobile/android/core/src/main/kotlin/org/mixyard/mobile/Endpoint.kt" "$task_root/verification/mobile-host/kotlin/build/production/"
cmake -S "$task_root/apps/mobile/native" -B "$task_build" \
  -DMIXYARD_ENGINE_ROOT="$1" -DMIXYARD_ENGINE_LIBRARY="$2"
cmake --build "$task_build" --parallel 2
GRADLE_USER_HOME="$RUNNER_TEMP/mobile-host-gradle" gradle --no-daemon -p "$task_root/verification/mobile-host/kotlin" installDist
java -Djava.library.path="$task_build" \
  -cp "$task_root/verification/mobile-host/kotlin/build/install/MixyardMobileHostQualification/lib/*" \
  org.mixyard.mobile.HostQualificationKt "$3"
