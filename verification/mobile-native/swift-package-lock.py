"""Check real independent CLI/Xcode resolver locks. Never create or resolve locks."""
import argparse
import hashlib
import json
from pathlib import Path

REVISION = '9f542610331815e29cc3821d3b6f488db8715517'
URL = 'https://github.com/apple/swift-asn1.git'


def verify(path):
    value = json.loads(path.read_text())
    pins = value.get('pins')
    if value.get('version') not in (2, 3) or not isinstance(pins, list) or len(pins) != 1:
        raise RuntimeError('Expected exactly one resolver-produced SwiftASN1 pin')
    pin = pins[0]
    if pin.get('identity') != 'swift-asn1' or pin.get('kind') != 'remoteSourceControl' or pin.get('location') != URL:
        raise RuntimeError('Unexpected Swift package identity/origin/kind')
    state = pin.get('state')
    if not isinstance(state, dict) or state.get('revision') != REVISION or state.get('branch') is not None:
        raise RuntimeError('Unexpected SwiftASN1 revision or floating branch')
    if state.get('version') not in (None, '1.6.0'):
        raise RuntimeError('Unexpected SwiftASN1 version')
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--xcode', action='store_true')
    args = parser.parse_args()
    package = Path(__file__).resolve().parents[2] / 'apps/mobile/ios'
    lock = package / 'Package.resolved'
    if not lock.is_file():
        raise RuntimeError('Hosted resolver preparation required; qualification is blocked until its real lock is committed')
    digest = verify(lock)
    xcode_digest = None
    if args.xcode:
        destination = package / 'Mixyard.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved'
        # Xcode's own resolver-produced representation is committed separately.
        # Its origin hash/schema may differ; the exact single dependency cannot.
        xcode_digest = verify(destination)
    print('Mobile trust lock: ' + json.dumps({'identity':'swift-asn1','revision':REVISION,'cliLockSha256':digest,'xcodeLockSha256':xcode_digest,'pins':1}))


if __name__ == '__main__':
    main()
