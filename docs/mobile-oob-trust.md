# Mobile OOB certificate trust contract

This separate production change is stacked above PR92, currently4560f0b. It is
not merged into78/92/93. After actual platform proof and independent whole review,
root sequences canonical integration/retargets; no entire92-history merge into78
or duplicate family is authorized. PR91 is accepted and unchanged.

The native pairing UI takes an exact HTTPS endpoint, an independently supplied
expected device ID, the owner's public trust JSON, and a separate invitation:

```json
{"deviceId":"64 lowercase hex characters","serverName":"mixyard-control","certificatePem":"one PEM certificate","certificateSha256":"64 hex characters"}
```

The record is at most64KiB and has exactly these four string fields. Both native
readers reject duplicates (including escaped-equivalent keys), unknown/missing
fields, nonstrings, trailing JSON and invalid JSON escapes. They compare the
record deviceId to the independently entered expected device before invitation
HTTP; the expected identity is never filled from that pasted record. One bounded
quoted-token envelope retains key multiplicity, delegating string decoding to
platform JSON. This is not a DER parser or cryptographic implementation.

The exact PEM must decode to one exact certificate DER with the declared SHA256.
The canonical name is mixyard-control. TLS also checks the actual endpoint name/IP.
The backend's random device ID is not signed into the certificate; this is an
explicit trusted OOB declaration of device association, not a cryptographic
certificate-to-device-ID derivation. Backend certificate generation is unchanged.

Android/JVM use the same shipping HTTP module: a fresh KeyStore contains only
that certificate; the platform TrustManager performs path checks, explicit public
X509 validity covers the leaf-as-anchor case, the exact leaf DER must match, and
OkHttp's normal hostname verifier remains installed. The canonical name must be
an exact DNS SAN. Swift configures the actual URLSession serverTrust per challenge
with public SSL policies for endpoint/canonical name, BasicX509, only the supplied
anchor and no AIA fetching. It also requires the exact leaf DER. Both preserve
TLS1.3, no proxy/redirect/auth followup/POST replay, existing HTTP limits and raw
canonical request/reply strings. No certificate comes from a network response.

Public SDK behavior MUST prove expired, future and self-signed bad-signature
anchors are rejected. The minimum remains iOS16/macOS13. No iOS18/macOS15-only
date API, custom DER parser, trust exception, global trust edit or system-anchor
fallback is used. If a public platform accepts a forbidden case, qualification
fails and root receives the exact SDK result/options before any parser/dependency
or support-floor change. Source preparation is not proof these cases pass.

Version2 secure-store records bind endpoint/device/certificate/name/hash to the
credential. Legacy records decode without erasure but cannot send HTTP; the UI
asks for OOB reprovisioning while retaining local credentials/drafts. A new
invitation is used only by an explicit Pair action; no automatic migration/replay.
Vault platform mechanisms remain Android Keystore-encrypted preferences and iOS
Keychain. Their physical/hardware/device-lock behavior remains separately unproved.

## Actual qualification and limits

The updated dedicated workflow compiles shipping Swift for macOS13 and compiles
the actual Android app with its existing minimum, without new dependencies/SDK
installation locally. The JVM qualifier uses the SAME HTTP module; Swift uses
SAME ControlClient/Security/URLSession. Only the vault boundary is doubled.
No user/admin/JVM truststore, Keychain search list, Security CLI or sudo trust
preparation executes. The old system-trust fixture is retained as FAILED
historical evidence: nine functional Swift passes did not clear its final owned
trust record; recovery and descendant limitations are not relabelled as success.

The original actual-service functional/raw Float64/authority/grants/401/409/
restart/no-replay/vault gates remain. A separate owned real-TLS matrix counts HTTP
requests for expired/future/bad-self-signature certs with matching private keys,
wrong OOB pin/anchor and TLS signature-algorithm mismatch, plus malformed/duplicate/
unknown/bounded OOB records and cross-device/name tuple mismatches. These must
reject before invitation HTTP. A legacy-vault probe requires zero credential HTTP
and a retained record. No external key or global trust edit fabricates a wire case.

A SEPARATELY LABELLED evaluator negative evaluates at most16 existing public system
certificates, requires one to actually pass ordinary platform trust, then requires
the shipping private-anchor provider to reject it. Unavailable evaluation fails;
identities/certificate dumps are never output. This is not an external system-
trusted leaf wire test. TLS signature-algorithm mismatch is not a forged TLS
CertificateVerify message, and no independent general CA-chain claim is made.
Those limits stay explicit for whole-contract acceptance.

Local verification only: strict TypeScript, owned certificate fixtures causally
verify time/self-signature defects and matching key, six process-lifecycle cases,
shell/YAML/diff checks. No local Kotlin/Swift/native/SDK build or execution. Current
actual platform results, missing-gap resolution and NEW independent whole-contract
review remain mandatory. Audio/worker consent/native availability flags unchanged.

Public API references: [Apple configuring trust](https://developer.apple.com/documentation/security/configuring-a-trust),
[anchor restriction](https://developer.apple.com/documentation/security/sectrustsetanchorcertificatesonly(_:_:)).
