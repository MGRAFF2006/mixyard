"""Proof-gate units only. Fake artifact bytes/Apple-tool boundary; no SDK run."""
import contextlib
import io
import os
from pathlib import Path
import plistlib
import tempfile
import time
import unittest
from architecture import admit_destination, verify_artifacts, remaining


def row(arch='arm64',**updates):
    value={'platform':'iOS Simulator','availability':'available','errorPresent':False,'duplicateFields':False,'archPresent':arch is not None,'arch':arch or 'missing-or-other'}
    value.update(updates);return value

def evidence(*rows):return {'records':list(rows),'ownedUuidMentioned':True,'recordLimitExceeded':False}


class ArchitectureProof(unittest.TestCase):
    def test_text_present_missing_wrong_and_duplicate_are_never_executable_proof(self):
        self.assertIn('still required',admit_destination(evidence(row()),'arm64'))
        self.assertIn('UNKNOWN',admit_destination(evidence(row(None)),'arm64'))
        # Explicit alternative architectures are normal available destinations;
        # the chosen native arch must still be executable-proved, not inferred.
        self.assertIn('still required',admit_destination(evidence(row(),row('x86_64')),'arm64'))
        for value in [evidence(row('x86_64')),evidence(row(),row()),evidence(row(duplicateFields=True)),evidence(row(errorPresent=True)),evidence(row(availability='ineligible')),evidence(row(platform='iOS'))]:
            with self.assertRaises(RuntimeError):admit_destination(value,'arm64')
    def fixture(self,root):
        products=root/'Build/Products/Debug-iphonesimulator'
        app=products/'Mixyard.app';runner=products/'MixyardConfirmations-Runner.app';tests=runner/'PlugIns/MixyardConfirmations.xctest'
        for bundle,name in [(app,'Mixyard'),(runner,'Runner'),(tests,'MixyardConfirmations')]:
            bundle.mkdir(parents=True,exist_ok=True)
            (bundle/'Info.plist').write_bytes(plistlib.dumps({'CFBundleExecutable':name}))
            (bundle/name).write_bytes(bytes.fromhex('cffaedfe')) # MOCK Mach-O header only.
        modules=products/'MobileCore.swiftmodule';modules.mkdir();(modules/'arm64-apple-ios-simulator.swiftmodule').write_bytes(b'mock-module')
        obj=root/'Build/Intermediates.noindex/Owned.build/Debug-iphonesimulator/MobileCore.build/Objects-normal/arm64/Endpoint.o'
        obj.parent.mkdir(parents=True);obj.write_bytes(bytes.fromhex('cffaedfe'))
        return obj,modules
    def test_positive_actual_product_roles_are_required_before_ui_admission(self):
        base='/dev/shm' if Path('/dev/shm').is_dir() else tempfile.gettempdir()
        with tempfile.TemporaryDirectory(prefix='owned-architecture-unit-',dir=base) as directory,contextlib.redirect_stdout(io.StringIO()) as output:
            root=Path(directory);self.fixture(root);calls=[]
            def apple(label,*args,**kwargs):calls.append((label,args,kwargs));return 'arm64'
            verify_artifacts(root,'arm64',time.monotonic()+5,apple)
            self.assertGreaterEqual(len(calls),4)
            self.assertTrue(all(args[:3]==('xcrun','lipo','-archs') for _,args,_ in calls))
            self.assertTrue(all(0<kwargs['timeout']<=5 for _,_,kwargs in calls))
            self.assertIn('"nativeOwnProducts": true',output.getvalue())
            self.assertNotIn(directory,output.getvalue())
    def test_wrong_compiled_arch_and_missing_core_binary_or_module_are_rejected(self):
        base='/dev/shm' if Path('/dev/shm').is_dir() else tempfile.gettempdir()
        for failure in ('wrong-app','wrong-core','missing-object','wrong-module'):
            with tempfile.TemporaryDirectory(prefix='owned-architecture-negative-',dir=base) as directory,contextlib.redirect_stdout(io.StringIO()):
                root=Path(directory);obj,modules=self.fixture(root)
                if failure=='missing-object':obj.unlink()
                if failure=='wrong-module':(modules/'x86_64-apple-ios-simulator.swiftmodule').write_bytes(b'mock')
                def apple(label,*args,**kwargs):return 'x86_64' if (failure=='wrong-app' and label=='compiled-app-architecture') or (failure=='wrong-core' and label=='compiled-mobile-core-architecture') else 'arm64'
                with self.assertRaises(RuntimeError):verify_artifacts(root,'arm64',time.monotonic()+5,apple)
    def test_expired_shared_budget_does_not_start_another_command(self):
        with self.assertRaises(RuntimeError):remaining(time.monotonic()-1,30)
