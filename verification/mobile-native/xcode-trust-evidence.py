"""Hosted proof of the actual shipping App's static supplementary trust target."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
from importlib.util import spec_from_file_location, module_from_spec


def tool(*args):
    return subprocess.check_output(args, text=True, stderr=subprocess.PIPE, timeout=20)


def main():
    output = Path(sys.argv[1]).resolve()
    arch = sys.argv[2]
    if arch not in ('arm64','x86_64'):
        raise RuntimeError('Unrecognized native architecture')
    package = Path(__file__).resolve().parents[2] / 'apps/mobile/ios'
    spec = spec_from_file_location('trust_lock', Path(__file__).with_name('swift-package-lock.py'))
    module = module_from_spec(spec); spec.loader.exec_module(module)
    digest = module.verify(package / 'Package.resolved')
    xcode_digest = module.verify(package / 'Mixyard.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved')
    checkout = output / 'SourcePackages/checkouts/swift-asn1'
    if tool('git','-C',str(checkout),'rev-parse','HEAD').strip() != module.REVISION:
        raise RuntimeError('Built parser checkout is not the exact pinned revision')
    if tool('git','-C',str(checkout),'status','--porcelain').strip():
        raise RuntimeError('Built parser checkout was modified')
    counts = {}
    for name in ('MobileControlTrust','SwiftASN1'):
        modules = list(output.glob('Build/Products/**/' + name + '.swiftmodule/*.swiftmodule'))
        if not modules or any(path.stem != arch+'-apple-ios-simulator' for path in modules):
            raise RuntimeError('Actual trust/parser module missing or conflicting')
        objects = list(output.glob('Build/Intermediates.noindex/**/'+name+'.build/Objects-normal/*/*.o'))
        if not objects or len(objects)>64:
            raise RuntimeError('Actual trust/parser object proof missing or unbounded')
        for obj in objects:
            if tool('xcrun','lipo','-archs',str(obj)).split() != [arch]:
                raise RuntimeError('Actual trust/parser object architecture differs')
        counts[name] = {'modules':len(modules),'objects':len(objects)}
    app = output / 'Build/Products/Debug-iphonesimulator/Mixyard.app'
    binaries = [app/'Mixyard'] + list(app.glob('*.debug.dylib'))
    if not any(any('CanonicalControlCertificate' in line and ' T ' in line for line in tool('xcrun','nm','-g',str(path)).splitlines()) for path in binaries):
        raise RuntimeError('App does not define the actual shared trust verifier')
    for path in binaries:
        links = tool('xcrun','otool','-L',str(path))
        if any(name in links for name in ('MobileControlTrust.framework/','SwiftASN1.framework/','libMobileControlTrust.','libSwiftASN1.')):
            raise RuntimeError('Unexpected dynamic trust/parser linkage')
    # Minimum deployment is executable evidence, not a source-setting claim.
    build = tool('xcrun','vtool','-show-build',str(app/'Mixyard'))
    if not any(line.strip() in ('minos 16.0','minos 16.0.0') for line in build.splitlines()):
        raise RuntimeError('Shipping App minimum deployment target is not iOS16')
    print('Mobile App trust evidence: ' + json.dumps({'staticVerifierDefined':True,
        'exactParserRevision':module.REVISION,'cliLockSha256':digest,'xcodeLockSha256':xcode_digest,'modulesAndObjects':counts,
        'minimumIosTarget':'16.0','sourceSha256':hashlib.sha256((package/'Sources/MobileControlTrust/CanonicalControlCertificate.swift').read_bytes()).hexdigest()}))


if __name__ == '__main__':
    main()
