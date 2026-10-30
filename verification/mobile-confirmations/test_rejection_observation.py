"""Controlled fixture/owned Python process tests, not Simulator or SDK proof."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import time
import unittest
from unittest.mock import patch
import rejection_observation as observation

OWNED='11111111-2222-3333-4444-555555555555'
OTHER='22222222-2222-3333-4444-555555555555'
RUNTIME='com.apple.CoreSimulator.SimRuntime.iOS-18-5'
TYPE='com.apple.CoreSimulator.SimDeviceType.iPhone-16'


def device(**updates):
    value={'udid':OWNED,'state':'Shutdown','isAvailable':True,'deviceTypeIdentifier':TYPE,'name':'SECRET_NAME'}
    value.update(updates);return value


def data(devices=None,group=RUNTIME):
    return json.dumps({'devices':{group:[device()] if devices is None else devices}}).encode()


class CatalogueClassification(unittest.TestCase):
    def test_three_hypothetical_catalogues_now_differ_without_identity_output(self):
        first='Available destinations for SECRET:\n'+'\n'.join([
            '{ platform:iOS, id:dvtdevice-DVTiPhonePlaceholder-iphoneos:placeholder, name:SECRET }',
            '{ platform:iOS Simulator, id:dvtdevice-DVTiOSDeviceSimulatorPlaceholder-iphonesimulator:placeholder, name:SECRET }',
            '{ platform:macOS, arch:arm64, id:OTHER, name:SECRET }'])
        second='Available destinations for SECRET:\n'+'\n'.join('{ platform:iOS Simulator, arch:arm64, id:'+str(i)*8+'-2222-3333-4444-555555555555, name:SECRET }' for i in [2,3,4])
        a=observation.classify_catalogue(first,OWNED);b=observation.classify_catalogue(second,OWNED)
        self.assertNotEqual(a,b);self.assertTrue(a['completeSourceScan'])
        self.assertEqual([r['idKind'] for r in a['records']],['known-placeholder-marker','known-placeholder-marker','other'])
        self.assertTrue(all(r['idKind']=='uuid-other' for r in b['records']))
        encoded=json.dumps([a,b]);self.assertNotIn('SECRET',encoded);self.assertNotIn(OTHER,encoded)
    def test_record_source_field_and_format_limits_are_explicitly_incomplete(self):
        line='{ platform:iOS Simulator, id:'+OWNED+', arch:arm64, arch:arm64, error:SECRET /PRIVATE }'
        value=observation.classify_catalogue('Ineligible destinations for Owned:\n'+line,OWNED)
        self.assertEqual(value['records'][0]['section'],'ineligible')
        self.assertTrue(value['records'][0]['errorPresent']);self.assertTrue(value['records'][0]['duplicateFields'])
        for text,reason in [('\n'.join([line]*9),'source-record-limit'),('x'*(observation.MAX_SOURCE_BYTES+1),'source-byte-limit'),
                            ('\n'*1025+'x','source-line-limit'),('{ id:'+OWNED+', '+','.join(['x:0']*33)+' }','source-field-limit'),
                            ('{ platform:iOS Simulator,\n id:'+OWNED+' }','source-brace-format')]:
            value=observation.classify_catalogue(text,OWNED)
            self.assertEqual(value['status'],'UNKNOWN');self.assertEqual(value['reason'],reason)
            self.assertFalse(value['completeSourceScan']);self.assertLessEqual(len(value['records']),8)


class ExactOwnedState(unittest.TestCase):
    def inspect(self,raw):return observation.inspect_device_json(raw,OWNED,RUNTIME,TYPE)
    def test_exact_available_mismatch_unavailable_and_missing_optional_type(self):
        result=self.inspect(data())
        self.assertEqual(result['status'],'OBSERVED');self.assertEqual(result['runtimeGroup'],'MATCH')
        self.assertEqual(result['deviceType'],'MATCH');self.assertIs(result['isAvailable'],True)
        result=self.inspect(data([device(deviceTypeIdentifier='other',isAvailable=False,availabilityError='SECRET_ERROR')],group=RUNTIME+'-other'))
        self.assertEqual(result['runtimeGroup'],'MISMATCH');self.assertEqual(result['deviceType'],'MISMATCH')
        self.assertIs(result['isAvailable'],False);self.assertIs(result['availabilityErrorPresent'],True)
        self.assertNotIn('SECRET',json.dumps(result))
        missing=device();missing.pop('deviceTypeIdentifier')
        result=self.inspect(data([missing]));self.assertEqual(result['status'],'UNKNOWN')
        self.assertEqual(result['deviceType'],'UNKNOWN') # Never a fabricated mismatch.
    def test_absent_duplicate_and_schema_partial_overflow_fail_unknown(self):
        result=self.inspect(data([]));self.assertEqual(result['reason'],'owned-absent')
        self.assertEqual(result['exactOwnedCount'],0);self.assertEqual(result['isAvailable'],'UNKNOWN')
        result=self.inspect(data([device()]*3));self.assertEqual(result['reason'],'owned-duplicate')
        self.assertEqual(result['exactOwnedCount'],2);self.assertTrue(result['matchCountCapped'])
        invalid=[b'{',b'{}',b'{"devices":[]} ',b'{"devices":{},"devices":{}}',b'{"devices":{}} trailing',
                 b'{"devices":{"x":[]},"extra":NaN}',data([{}]),data([device(udid='not-uuid')]),
                 data([device(isAvailable='true')]),data([device(state=1)]),b'\xff',b' '*(observation.MAX_STDOUT+1)]
        for raw in invalid:
            with self.subTest(length=len(raw)):
                result=self.inspect(raw);self.assertEqual(result['status'],'UNKNOWN')
                self.assertEqual(result['exactOwnedCount'],'UNKNOWN')
        records=[device(udid=f'{i:08x}-2222-3333-4444-555555555555') for i in range(257)]
        self.assertEqual(self.inspect(data(records))['reason'],'entry-limit')
        missing=device();missing.pop('isAvailable');result=self.inspect(data([missing]))
        self.assertEqual(result['status'],'UNKNOWN');self.assertEqual(result['isAvailable'],'UNKNOWN')


class BoundedOwnChild(unittest.TestCase):
    def command(self,code):return [sys.executable,'-B','-c',code]
    def read(self,code):
        with patch.object(observation,'STOP_RESERVE',0.1):
            start=time.monotonic()
            result=observation.read_devices(start+0.5,self.command(code))
            self.assertLess(time.monotonic()-start,2) # Scheduling tolerance, not a realtime guarantee.
            return result
    def test_complete_owned_python_child_and_nonzero_partial_stderr(self):
        raw,receipt=self.read('import sys;sys.stdout.write('+repr(data().decode())+')')
        self.assertEqual(raw,data());self.assertEqual(receipt['status'],'COMPLETE')
        self.assertTrue(receipt['directChildReaped']);self.assertTrue(receipt['pipesEof'])
        for code,reason in [('raise SystemExit(7)','tool-exit'),('import sys;sys.stderr.write("SECRET_ERROR")','stderr-present')]:
            raw,receipt=self.read(code);self.assertIsNone(raw);self.assertEqual(receipt['reason'],reason)
            self.assertTrue(receipt['directChildReaped']);self.assertNotIn('SECRET',json.dumps(receipt))
        raw,receipt=self.read('print("{")')
        self.assertEqual(receipt['status'],'COMPLETE')
        self.assertEqual(observation.inspect_device_json(raw,OWNED,RUNTIME,TYPE)['status'],'UNKNOWN')
    def test_timeout_caps_and_retained_ownership_are_unknown_without_retry(self):
        raw,receipt=self.read('import time;time.sleep(10)')
        self.assertIsNone(raw);self.assertEqual(receipt['reason'],'timeout')
        self.assertTrue(receipt['directChildReaped']);self.assertFalse(receipt['ownedChildRetained'])
        for field,code,reason in [('MAX_STDOUT','import sys;sys.stdout.write("x"*10000)','stdout-limit'),
                                  ('MAX_STDERR','import sys;sys.stderr.write("SECRET"*10000)','stderr-limit')]:
            with patch.object(observation,field,128):raw,receipt=self.read(code)
            self.assertIsNone(raw);self.assertEqual(receipt['reason'],reason)
            self.assertTrue(receipt['directChildReaped']);self.assertNotIn('SECRET',json.dumps(receipt))
        with patch.object(observation,'_HELD_CHILD',object()),patch.object(observation.subprocess,'Popen') as spawn:
            raw,receipt=observation.read_devices(time.monotonic()+3,self.command('pass'))
        spawn.assert_not_called();self.assertTrue(receipt['ownedChildRetained'])
        self.assertEqual(receipt['status'],'UNKNOWN')
    def test_failed_or_incomplete_capture_cannot_publish_valid_looking_payload(self):
        for reason in ['timeout','tool-exit','stdout-limit','stderr-limit','stop-incomplete']:
            receipt={'status':'UNKNOWN','reason':reason,'directChildReaped':False,'pipesEof':False}
            output=io.StringIO()
            with patch.object(observation,'read_devices',return_value=(data(),receipt)) as read, \
                 patch.object(observation,'inspect_device_json') as inspect,contextlib.redirect_stdout(output):
                observation.observe_rejection('',OWNED,RUNTIME,TYPE)
            read.assert_called_once();inspect.assert_not_called()
            value=json.loads(output.getvalue().split(': ',1)[1])
            self.assertEqual(value['ownedState']['status'],'UNKNOWN')
            self.assertEqual(value['ownedState']['exactOwnedCount'],'UNKNOWN')
            self.assertEqual(value['ownedState']['isAvailable'],'UNKNOWN')
            self.assertEqual(value['ownedState']['reason'],reason)

    def test_one_read_receipt_is_redacted_and_consumes_selected_expected_values(self):
        complete={'status':'COMPLETE','reason':'ok','directChildReaped':True,'pipesEof':True}
        output=io.StringIO()
        with patch.object(observation,'read_devices',return_value=(data([device(availabilityError='SECRET')]),complete)) as read,contextlib.redirect_stdout(output):
            start=time.monotonic();observation.observe_rejection('{ id:'+OWNED+' }',OWNED,RUNTIME,TYPE)
        read.assert_called_once();self.assertLessEqual(read.call_args.args[0]-start,3.01)
        self.assertEqual(len(output.getvalue().splitlines()),1)
        value=json.loads(output.getvalue().split(': ',1)[1])
        self.assertEqual(value['ownedState']['runtimeGroup'],'MATCH')
        self.assertEqual(value['ownedState']['deviceType'],'MATCH')
        for token in [OWNED,'SECRET','/PRIVATE',RUNTIME,TYPE]:self.assertNotIn(token,output.getvalue())


class AdmissionIntegration(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec=importlib.util.spec_from_file_location('failure_runner',Path(__file__).with_name('run-ios.py'))
        cls.runner=importlib.util.module_from_spec(spec);spec.loader.exec_module(cls.runner)
    def exercise(self,rejection=None,probe_error=None,cleanup_errors=False):
        module=self.runner;events=[];requested={};output=io.StringIO()
        def capture(label,*args,**kwargs):
            events.append(label)
            if label=='selected-xcode':return 'Xcode 16.4'
            if label=='selected-sdk':return '18.5'
            if label=='runtime-discovery':return json.dumps({'runtimes':[{'identifier':RUNTIME,'isAvailable':True,'version':'18.5'}]})
            if label=='owned-create':requested['type']=args[-2];requested['runtime']=args[-1];return OWNED
            if label=='owned-destinations':return 'Available destinations for Owned:\n{ platform:iOS Simulator, arch:arm64, id:'+OWNED+' }'
            raise AssertionError('Unexpected capture')
        def step(label,args,timeout):
            events.append(label)
            if cleanup_errors and label in ('cleanup-shutdown','cleanup-delete'):raise subprocess.TimeoutExpired('owned-cleanup',timeout)
        def probe(text,owned,runtime,device_type):
            events.append('probe');self.assertEqual((owned,runtime,device_type),(OWNED,requested['runtime'],requested['type']))
            if probe_error:raise probe_error
        with patch.dict(module.os.environ,{'GITHUB_ACTIONS':'true','RUNNER_OS':'macOS','RUNNER_TEMP':'/owned'}), \
             patch.object(module.platform,'machine',return_value='arm64'),patch.object(module,'capture',side_effect=capture), \
             patch.object(module,'step',side_effect=step),patch.object(module,'toolchain_evidence'),patch.object(module,'process_evidence'), \
             patch.object(module,'verify_artifacts'),patch.object(module,'observe_rejection',side_effect=probe) as observe, \
             patch.object(module,'admit_destination',side_effect=rejection,return_value='admitted'),contextlib.redirect_stdout(output):
            if rejection:
                with self.assertRaises(RuntimeError) as caught:module.main()
                self.assertIs(caught.exception,rejection)
            else:module.main()
        return events,observe.call_count,output.getvalue()
    def test_success_has_original_sdk_order_and_no_probe(self):
        events,count,_=self.exercise()
        self.assertEqual(count,0)
        self.assertEqual(events,['locked-trust-package','selected-xcode','selected-sdk','runtime-discovery','owned-create',
            'locked-package-resolution','owned-destinations','boot','bootstatus','actual-build-for-testing',
            'actual-shared-trust-link-proof','actual-ui-test','cleanup-shutdown','cleanup-delete'])
    def test_same_original_rejection_survives_probe_success_failure_and_cleanup(self):
        for problem in [None,RuntimeError('SECRET_DIAGNOSTIC'),SystemExit('SECRET_EXIT')]:
            rejection=RuntimeError('original-admission')
            events,count,output=self.exercise(rejection,problem,cleanup_errors=True)
            self.assertEqual(count,1);self.assertNotIn('SECRET',output)
            self.assertEqual(events[-3:],['probe','cleanup-shutdown','cleanup-delete'])
            self.assertNotIn('boot',events);self.assertNotIn('actual-build-for-testing',events)
            self.assertEqual(len(rejection.__cause__.exceptions),2)
