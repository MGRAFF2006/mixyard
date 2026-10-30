#!/usr/bin/env bash
set -euo pipefail
task_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
: "${MIXYARD_NATIVE_FIXTURE:?Owned fixture directory is required}"
# Shipping HTTP module constructs only its single OOB-anchor trust manager.
# No JVM default truststore override, certificate import or global trust edit.
exec java -cp "$task_root/kotlin/build/install/MixyardNativeControlQualification/lib/*" \
  org.mixyard.mobile.NativeQualificationKt
