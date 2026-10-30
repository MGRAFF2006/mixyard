"""Positive static-link/source/pin/minimum-target evidence from hosted products."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
from defined_swift_symbols import attest_defined


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    build, binary, package = map(Path, sys.argv[1:])
    archive = binary / 'libMobileControlTrust.a'
    executable = build / 'native-control-qualification'
    # Defined supplementary verifier must actually be linked into BOTH the
    # product archive and the final qualifier (not merely present in metadata).
    attest_defined([archive], 'trust-archive')
    attest_defined([executable], 'qualifier')
    dependencies = subprocess.check_output(['xcrun','otool','-L',str(executable)], text=True, timeout=20)
    if any(name in dependencies for name in ('libMobileControlTrust.', 'libSwiftASN1.')):
        raise RuntimeError('Unexpected dynamic trust/parser dependency')
    build_version = subprocess.check_output(['xcrun','vtool','-show-build',str(executable)], text=True, timeout=20)
    if not any(line.strip() in ('minos 13.0','minos 13.0.0') for line in build_version.splitlines()):
        raise RuntimeError('Qualifier minimum deployment target is not macOS13')
    checkout = build / 'package/checkouts/swift-asn1'
    revision = subprocess.check_output(['git','-C',str(checkout),'rev-parse','HEAD'], text=True, timeout=20).strip()
    if revision != '9f542610331815e29cc3821d3b6f488db8715517':
        raise RuntimeError('Actual parser checkout differs from exact pin')
    if subprocess.check_output(['git','-C',str(checkout),'status','--porcelain'], text=True, timeout=20).strip():
        raise RuntimeError('Actual parser checkout was modified')
    source_files = sorted((package / 'Sources/MobileControlTrust').glob('*.swift'))
    print('Mobile trust static-link evidence: ' + json.dumps({
        'archiveSha256':digest(archive),'qualifierSha256':digest(executable),
        'sourceSha256':[digest(path) for path in source_files],
        'lockSha256':digest(package / 'Package.resolved'),'actualParserRevision':revision,
        'minimumMacTarget':'13.0','staticTrustAndASN1':True}))


if __name__ == '__main__':
    main()
