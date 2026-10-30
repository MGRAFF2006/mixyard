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


## Exact5d actual results and direct repair checkpoint

Root independently read5d jobs123923635166 and123923635465/run41013242619.
Kotlin compiled, its original nine functional cases passed, and the actual
bad-self-signature case sent one HTTP request (the sink500 prevented native success).
Expired/future cases rejected with one TLS connection/zero HTTP; the system
certificate evaluator passed. This is a proven anchor-signature gap, not a test
success. The canonical backend explicitly generates a self-signed identity.
Android now requires equal public issuer/subject principals, public
[`Certificate.verify`](https://docs.oracle.com/en/java/javase/17/docs/api/java.base/java/security/cert/Certificate.html#verify(java.security.PublicKey)) with the certificate public key, one exact pinned peer certificate,
and retains platform TrustManager/time/hostname/TLS1.3. General CA-issued paths
are explicitly unsupported; no DER/crypto parser or algorithm implementation.
The unchanged matching-key bad-self-signature before-HTTP assertion must pass
on the new actual shipping execution before acceptance.

Swift5d compiled but valid pairing failed preflight (MobileError), all negative
TLS connection counts were zero, and the evaluator was unavailable. Those
negative counts provide NO public-policy credit. Fixed reason/phase allowlists
now distinguish record/certificate/native/platform failure, never associated
unknown data, arbitrary error descriptions or provisioning. The PEM Character
filter contained CRLF as a grapheme, potentially missing lone LF. ASCII byte
whitespace removal preserves the intended exact base64/DER/hash guards. Hosted
qualification compiles exact5d ControlClient into a SEPARATE target and requires
a causal LF failure with CRLF/redaction success, then compiles CURRENT shipping
source and requires LF+CRLF positives before TLS. Until actual execution verifies
that mechanism, it remains a source-supported diagnosis, not a claimed SDK fix.
If Swift later accepts a forbidden signature/date case, stop and report the exact
public SDK gap before any parser/dependency/minimum-target change.


## Supplementary Swift canonical self-signature target (preparatory candidate)

Root selected SwiftASN1 **1.6.0**, exact revision
`9f542610331815e29cc3821d3b6f488db8715517`, with public Security/SecKey.
`MobileControlTrust` is one static product in the existing local iOS package;
MobileCore retains no parser or Security dependency. The shipping App and actual
ControlClient qualifier use that SAME target, not copies of parser/client code.
SwiftASN1's selected manifest has no package dependencies. Actual resolution,
linkage and deployment verification are mandatory hosted gates, not inferred
from that source metadata. No local SwiftPM/SDK resolution/build occurred.

The supplement accepts only the canonical backend self-signed v3 P256/SHA256
profile. Typed DER parsing retains the ORIGINAL complete TBS TLV for
`SecKeyVerifySignature(...ecdsaSignatureMessageX962SHA256...)`; there is no
reserialization/prehash, custom crypto/curve math or low-S policy. Inner and
outer signature AlgorithmIdentifiers must agree exactly, have ecdsa-with-SHA256
OID and absent parameters. SPKI must declare id-ecPublicKey/prime256v1; public
SecKey class/type/256-bit size/algorithm support must agree. Normalized public
issuer/subject must be nonnull, nonempty and equal. Signature BITSTRING must be
primitive, nonempty and zero-padding, containing exactly two minimal positive,
nonzero, at-most256-bit DER integers; SecKey also checks cryptographic ranges.
Unknown profiles/algorithms/curves reject; this is not general CA-path support.
The normal endpoint/canonical-name SSL, BasicX509, dates, Only-anchor, no-AIA,
TLS1.3 and exact leaf/pin checks remain. No date parser or iOS16/macOS13 floor
change is introduced.

Actual-adapter XCTest fixtures sign with public SecKey and cover valid canonical
certificates, all byte-truncation boundaries, root length/trailing/extra/depth
errors, algorithm/parameter/curve/key/name/profile mismatches, and malformed,
negative/zero/overlong/extra signature integers and BITSTRING forms. These are
SOURCE-PREPARED tests, not executed SDK proof at this preparatory head. The
existing actual-TLS before-HTTP bad-self-signature gate and full31-test matrix
remain unchanged.

**Lock preparation boundary:** no lock contents were invented. The first hosted
`swift-resolver-lock` job generates real CLI and Xcode resolver locks and uploads
`mobile-control-trust-resolver-locks`, then deliberately fails the preparation
gate. Those two actual files must be committed before subsequent locked
current-head qualification can run. App/qualifier builds require exactly one
resolved dependency at the selected revision, no floating branch/transitive
expansion, and report real checkout/source/module/archive/executable identities.
Both Xcode16.2 and16.4 TLS qualifier lanes and both actual iOS UI SDK lanes remain
required. The minimum-target/static-link gates inspect actual built products.
No binary-size, compile-time or runtime-cost measurement is claimed.

Historical a70 evidence remains distinct: Kotlin31/31 passed after the JCA
self-signature preflight; Swift valid positives and date negatives ran, but its
matching-key bad-self-signature sent one HTTP request and failed. Exact5d LF RED
and a70 LF+CRLF GREEN were actually executed; that old source-layout gate is not
rerun against a new module layout to manufacture a compile-error RED. Old
system-trust cleanup failures remain failures. This new candidate has no current
native proof or independent final approval; PR113 remains DRAFT/HOLD.
