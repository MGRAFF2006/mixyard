"""Positive built-product architecture proof; no absent-text inference."""
import hashlib
import json
from pathlib import Path
import plistlib
import re
import time


def admit_destination(evidence, arch):
    rows=evidence['records']
    if not evidence['ownedUuidMentioned'] or evidence['recordLimitExceeded'] or not rows or any(
        row['platform']!='iOS Simulator' or row['availability']!='available' or row['errorPresent'] or row['duplicateFields'] for row in rows):
        raise RuntimeError('Owned simulator destination is unavailable or ambiguous')
    advertised=[row for row in rows if row['archPresent'] and row['arch']==arch]
    if len(advertised)==1: return 'advertised; built proof still required'
    if len(advertised)>1 or any(row['archPresent'] for row in rows) or len(rows)!=1:
        raise RuntimeError('Owned simulator advertised architecture is wrong or ambiguous')
    return 'UNKNOWN; positive built-product proof required'


def remaining(deadline, maximum):
    value=min(maximum,deadline-time.monotonic())
    if value<=0: raise RuntimeError('Owned build/proof/UI900s budget exhausted')
    return value


def verify_artifacts(output, arch, deadline, capture):
    root=Path(output).resolve();products=root/'Build/Products/Debug-iphonesimulator'
    app=products/'Mixyard.app'
    runner=products/'MixyardConfirmations-Runner.app'
    tests=runner/'PlugIns/MixyardConfirmations.xctest'
    def executable(bundle):
        try:
            name=plistlib.loads((bundle/'Info.plist').read_bytes())['CFBundleExecutable']
            if not isinstance(name,str) or not re.fullmatch(r'[A-Za-z0-9_.-]{1,128}',name):raise ValueError()
            value=bundle/name
            if not value.is_file():raise ValueError()
            return value
        except (OSError,ValueError,KeyError,plistlib.InvalidFileException):
            raise RuntimeError('Owned built app/test executable proof missing') from None
    artifacts=[('app',executable(app),True),('ui-test',executable(tests),True),('ui-runner',executable(runner),False)]
    modules=list(root.glob('Build/Products/**/MobileCore.swiftmodule/*.swiftmodule'))
    triple=arch+'-apple-ios-simulator'
    if not modules or any(path.stem!=triple for path in modules):
        raise RuntimeError('Built MobileCore native simulator module proof missing/conflicting')
    objects=list(root.glob('Build/Intermediates.noindex/**/MobileCore.build/Objects-normal/*/*.o'))
    aggregate=products/'MobileCore.o'
    if aggregate.is_file():objects.append(aggregate)
    framework=products/'PackageFrameworks/MobileCore.framework/MobileCore'
    if framework.is_file():objects.append(framework)
    if not objects or len(objects)>128:
        raise RuntimeError('Built MobileCore binary/object architecture proof missing/bounded')
    artifacts.extend(('mobile-core',path,True) for path in objects)
    # Inspect all actually packaged Mach-O dependencies as well. SDK libraries
    # may be universal but must contain the selected native architecture.
    magic={bytes.fromhex(value) for value in ['feedface','cefaedfe','feedfacf','cffaedfe','cafebabe','bebafeca','cafebabf','bfbafeca']}
    for bundle in (app,runner):
        for path in bundle.rglob('*'):
            if not path.is_file():continue
            with path.open('rb') as stream:header=stream.read(4)
            if header in magic:artifacts.append(('packaged',path,False))
    if len(artifacts)>256:raise RuntimeError('Owned Mach-O proof bound exceeded')
    checked={};counts={'app':0,'ui-test':0,'ui-runner':0,'mobile-core':0,'packaged':0};hashes={}
    for role,path,exact in artifacts:
        resolved=path.resolve()
        if not resolved.is_relative_to(root):raise RuntimeError('Built artifact escaped owned products')
        if resolved not in checked:
            try:raw=capture('compiled-'+role+'-architecture','xcrun','lipo','-archs',str(resolved),timeout=remaining(deadline,30))
            except Exception:raise RuntimeError('Apple tool could not verify owned '+role+' architecture') from None
            values=raw.split()
            if not values or len(values)>4 or len(set(values))!=len(values) or any(value not in ('arm64','arm64e','x86_64','i386') for value in values):
                raise RuntimeError('Unrecognized Mach-O architecture proof')
            checked[resolved]=set(values)
        values=checked[resolved]
        if arch not in values or (exact and values!={arch}):raise RuntimeError('Built '+role+' architecture conflicts with selected native target')
        counts[role]+=1
        if role in ('app','ui-test'):
            digest=hashlib.sha256()
            with resolved.open('rb') as stream:
                for data in iter(lambda:stream.read(65536),b''):
                    remaining(deadline,30);digest.update(data)
            hashes[role]=digest.hexdigest()
    print('Owned compiled architecture proof: '+json.dumps({'arch':arch,'nativeOwnProducts':True,'roles':counts,'appTestSha256':hashes,'mobileCoreModules':len(modules)}),flush=True)
