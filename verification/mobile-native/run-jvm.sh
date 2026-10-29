#!/usr/bin/env bash
set -euo pipefail
task_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
: "${MIXYARD_NATIVE_FIXTURE:?Owned fixture directory is required}"
task_store="$MIXYARD_NATIVE_FIXTURE/jvm-trust.p12"
for task_device in a b viewer; do
  keytool -importcert -noprompt -storetype PKCS12 -alias "owned-$task_device" \
    -file "$MIXYARD_NATIVE_FIXTURE/$task_device/certificate.pem" \
    -keystore "$task_store" -storepass fixture-only >/dev/null 2>&1 || {
      echo 'Owned JVM certificate provisioning failed' >&2; exit 1;
    }
done
exec java -Djavax.net.ssl.trustStore="$task_store" -Djavax.net.ssl.trustStorePassword=fixture-only \
  -cp "$task_root/kotlin/build/install/MixyardNativeControlQualification/lib/*" \
  org.mixyard.mobile.NativeQualificationKt
