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
import owned_simulator_state as observation

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
    def test_stop_reserve_is_inside_each_total_not_an_extra_wait(self):
        # Drive the actual reader to its IO deadline without sleeping; the only
        # remaining stop/reap allowance must end at the caller's original total.
        for total in (3,30,90):
            now=[100.0];waits=[]
            selector=unittest.mock.MagicMock()
            selector.get_map.return_value={1:object()}
            def select(timeout):
                now[0]+=timeout
                return []
            selector.select.side_effect=select
            process=unittest.mock.MagicMock()
            process.returncode=None;process.poll.return_value=None
            process.stdout.closed=False;process.stderr.closed=False
            process.stdout.close.side_effect=lambda:setattr(process.stdout,'closed',True)
            process.stderr.close.side_effect=lambda:setattr(process.stderr,'closed',True)
            def wait(timeout):
                waits.append(timeout);process.returncode=-9
                self.assertLessEqual(now[0]+timeout,100+total)
            process.wait.side_effect=wait
            with patch.object(observation.time,'monotonic',side_effect=lambda:now[0]), \
                 patch.object(observation.selectors,'DefaultSelector',return_value=selector), \
                 patch.object(observation.subprocess,'Popen',return_value=process),patch.object(observation.os,'set_blocking'):
                raw,receipt=observation.read_devices(100+total,['owned-controlled-child'])
            self.assertIsNone(raw);self.assertEqual(receipt['reason'],'timeout')
            selector.select.assert_called_once()
            self.assertAlmostEqual(selector.select.call_args.args[0],total-0.6)
            self.assertEqual(len(waits),1);self.assertAlmostEqual(waits[0],0.6)
            process.kill.assert_called_once()
            self.assertTrue(receipt['directChildReaped']);self.assertTrue(receipt['localPipesClosed'])
            self.assertFalse(receipt['pipesEof']);self.assertFalse(receipt['ownedChildRetained'])

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


def complete_receipt(raw):
    return {'status':'COMPLETE','reason':'ok','toolValue':0,'directChildReaped':True,
            'ownedChildRetained':False,'pipesEof':True,'localPipesClosed':True,
            'stdoutBytes':len(raw),'stderrBytes':0}


class RequiredBootedGate(unittest.TestCase):
    def exercise(self,raw,receipt=None,reject=False):
        output=io.StringIO();receipt=complete_receipt(raw) if receipt is None else receipt
        with patch.object(observation,'read_devices',return_value=(raw,receipt)) as read,contextlib.redirect_stdout(output):
            if reject:
                with self.assertRaisesRegex(RuntimeError,'Owned booted simulator precondition failed'):
                    observation.require_booted_simulator(OWNED,RUNTIME,TYPE)
            else:observation.require_booted_simulator(OWNED,RUNTIME,TYPE)
        read.assert_called_once()
        for secret in [OWNED,RUNTIME,TYPE,'SECRET','/PRIVATE']:self.assertNotIn(secret,output.getvalue())
        return json.loads(output.getvalue().split(': ',1)[1])
    def test_required_state_has_30s_while_legacy_observation_keeps_3s(self):
        raw=data([device(state='Booted')]);output=io.StringIO()
        with patch.object(observation.time,'monotonic',return_value=100), \
             patch.object(observation,'read_devices',return_value=(raw,complete_receipt(raw))) as read,contextlib.redirect_stdout(output):
            observation.require_booted_simulator(OWNED,RUNTIME,TYPE)
            observation.observe_rejection('',OWNED,RUNTIME,TYPE)
        self.assertEqual([call.args[0] for call in read.call_args_list],[130,103])
        lines=output.getvalue().splitlines()
        required=json.loads(lines[0].split(': ',1)[1])
        self.assertEqual(required['configuredSeconds'],30)
        self.assertTrue(required['preconditionSatisfied'])

    def test_only_complete_exact_owned_available_booted_matching_state_passes(self):
        result=self.exercise(data([device(state='Booted')]))
        self.assertTrue(result['preconditionSatisfied'])
        cases=[data([]),data([device(state='Booted')]*2),data([device(udid=OTHER,state='Booted')]),
            data([device(state='Shutdown')]),data([device(state='Booting')]),data([device(state='Booted',isAvailable=False)]),
            data([device(state='Booted',deviceTypeIdentifier='other')]),data([device(state='Booted')],group=RUNTIME+'-other'),
            data([device(state='Booted',availabilityError=None)]),b'{',b'{"devices":{},"devices":{}}',b'\xff']
        missing=device(state='Booted');missing.pop('deviceTypeIdentifier');cases.append(data([missing]))
        cases.append(data([device(udid=f'{i:08x}-2222-3333-4444-555555555555',state='Booted') for i in range(257)]))
        for raw in cases:
            with self.subTest(length=len(raw)):self.assertFalse(self.exercise(raw,reject=True)['preconditionSatisfied'])
    def test_capture_unknown_cap_tool_reap_eof_or_localclose_cannot_qualify(self):
        raw=data([device(state='Booted')])
        for update in [{'status':'UNKNOWN'},{'toolValue':7},{'directChildReaped':False},{'pipesEof':False},
                       {'localPipesClosed':False},{'ownedChildRetained':True},{'stdoutBytes':observation.MAX_STDOUT+1},{'stdoutBytes':0},{'stderrBytes':1}]:
            receipt=complete_receipt(raw);receipt.update(update)
            with self.subTest(fields=list(update)):self.assertFalse(self.exercise(raw,receipt,reject=True)['preconditionSatisfied'])
    def test_every_gate_reads_fresh_and_unexpected_errors_are_redacted(self):
        booted=data([device(state='Booted')]);stopped=data([device(state='Shutdown')])
        output=io.StringIO()
        with patch.object(observation,'read_devices',side_effect=[(booted,complete_receipt(booted)),(stopped,complete_receipt(stopped))]) as read,contextlib.redirect_stdout(output):
            observation.require_booted_simulator(OWNED,RUNTIME,TYPE)
            with self.assertRaises(RuntimeError):observation.require_booted_simulator(OWNED,RUNTIME,TYPE)
        self.assertEqual(read.call_count,2)
        with patch.object(observation,'read_devices',side_effect=RuntimeError('SECRET_PAYLOAD')),contextlib.redirect_stdout(output):
            with self.assertRaises(RuntimeError):observation.require_booted_simulator(OWNED,RUNTIME,TYPE)
        self.assertNotIn('SECRET',output.getvalue())


class ExactExecutionIntegration(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec=importlib.util.spec_from_file_location('state_runner',Path(__file__).with_name('run-ios.py'))
        cls.runner=importlib.util.module_from_spec(spec);spec.loader.exec_module(cls.runner)
    def exercise(self,state_raw=None,capture_receipt=None,action_failure=None,artifact_failure=None,cleanup_errors=False,expect_state_failure=False):
        module=self.runner;events=[];commands=[];requested={};output=io.StringIO()
        raw=data([device(state='Booted')]) if state_raw is None else state_raw
        receipt=complete_receipt(raw) if capture_receipt is None else capture_receipt
        def capture(label,*args,**kwargs):
            events.append(label)
            if label=='selected-xcode':return 'Xcode 16.4'
            if label=='selected-sdk':return '18.5'
            if label=='owned-create':requested['type']=args[-2];requested['runtime']=args[-1];return OWNED
            raise AssertionError('Unexpected capture, including retired textual enumeration')
        def runtimes():
            events.append('runtime-discovery')
            return json.dumps({'runtimes':[{'identifier':RUNTIME,'isAvailable':True,'version':'18.5'}]})
        def step(label,args,timeout):
            events.append(label);commands.append((label,args,timeout))
            if label=='boot':self.assertEqual((args,timeout),(['xcrun','simctl','boot',OWNED],30))
            if label=='bootstatus':self.assertEqual((args,timeout),(['xcrun','simctl','bootstatus',OWNED,'-b'],180))
            if action_failure and label==action_failure[0]:raise action_failure[1]
            if cleanup_errors and label in ('cleanup-shutdown','cleanup-delete'):raise subprocess.TimeoutExpired('owned-cleanup',timeout)
        def gate(owned,runtime,device_type):
            events.append('owned-state')
            self.assertEqual((owned,runtime,device_type),(OWNED,requested['runtime'],requested['type']))
            return observation.require_booted_simulator(owned,runtime,device_type)
        def artifacts(*_args):
            events.append('artifact-proof')
            if artifact_failure:raise artifact_failure
        with patch.dict(module.os.environ,{'GITHUB_ACTIONS':'true','RUNNER_OS':'macOS','RUNNER_TEMP':'/owned'}), \
             patch.object(module.platform,'machine',return_value='arm64'),patch.object(module,'capture',side_effect=capture), \
             patch.object(module,'capture_runtime_catalogue',side_effect=runtimes),patch.object(module,'step',side_effect=step), \
             patch.object(module,'toolchain_evidence'),patch.object(module,'process_evidence'), \
             patch.object(module,'verify_artifacts',side_effect=artifacts),patch.object(module,'require_booted_simulator',side_effect=gate), \
             patch.object(observation,'read_devices',return_value=(raw,receipt)) as read,contextlib.redirect_stdout(output):
            expected=action_failure[1] if action_failure else artifact_failure
            if expected:
                with self.assertRaises(type(expected)) as caught:module.main()
                self.assertIs(caught.exception,expected)
            elif expect_state_failure:
                with self.assertRaisesRegex(RuntimeError,'Owned booted simulator precondition failed'):module.main()
            else:module.main()
        return events,commands,read.call_count,output.getvalue()
    def test_valid_state_executes_exact_same_owned_selector_for_both_actions(self):
        events,commands,count,_=self.exercise()
        self.assertEqual(count,1)
        self.assertEqual(events,['locked-trust-package','selected-xcode','selected-sdk','runtime-discovery','owned-create',
            'locked-package-resolution','boot','bootstatus','owned-state','actual-build-for-testing',
            'artifact-proof','actual-shared-trust-link-proof','actual-ui-test','cleanup-shutdown','cleanup-delete'])
        actions=[args for label,args,_ in commands if label in ('actual-build-for-testing','actual-ui-test')]
        self.assertEqual(len(actions),2);self.assertEqual(actions[0][2:],actions[1][2:])
        self.assertEqual(actions[0][:2],['xcodebuild','build-for-testing'])
        self.assertEqual(actions[1][:2],['xcodebuild','test-without-building'])
        for args in actions:
            self.assertEqual(args[args.index('-destination')+1],'platform=iOS Simulator,id='+OWNED)
            self.assertIn('ARCHS=arm64',args);self.assertIn('ONLY_ACTIVE_ARCH=YES',args)
            self.assertIn('-onlyUsePackageVersionsFromResolvedFile',args)
        self.assertFalse(any('-showdestinations' in args for _,args,_ in commands))
    def test_wrong_missing_duplicate_unavailable_unbooted_or_unknown_state_prevents_build(self):
        cases=[data([]),data([device(state='Booted')]*2),data([device(udid=OTHER,state='Booted')]),
               data([device(state='Shutdown')]),data([device(state='Booted',isAvailable=False)]),
               data([device(state='Booted',deviceTypeIdentifier='other')]),data([device(state='Booted')],group=RUNTIME+'-other'),b'{']
        for raw in cases:
            events,_,count,output=self.exercise(state_raw=raw,expect_state_failure=True,cleanup_errors=True)
            self.assertEqual(count,1);self.assertNotIn('actual-build-for-testing',events)
            self.assertEqual(events[-2:],['cleanup-shutdown','cleanup-delete']);self.assertNotIn('SECRET',output)
        for changes in [{'pipesEof':False},{'stdoutBytes':observation.MAX_STDOUT+1},{'status':'UNKNOWN'},{'ownedChildRetained':True}]:
            receipt=complete_receipt(data([device(state='Booted')]));receipt.update(changes)
            events,_,_,_=self.exercise(capture_receipt=receipt,expect_state_failure=True)
            self.assertNotIn('actual-build-for-testing',events)
            self.assertEqual(events[-2:],['cleanup-shutdown','cleanup-delete'])
    def test_preparation_build_artifact_static_and_ui_failures_stay_fatal_with_both_cleanups(self):
        for label in ('boot','bootstatus','actual-build-for-testing','actual-shared-trust-link-proof','actual-ui-test'):
            failure=subprocess.CalledProcessError(70,'owned-'+label)
            events,_,count,_=self.exercise(action_failure=(label,failure),cleanup_errors=True)
            self.assertEqual(events[-2:],['cleanup-shutdown','cleanup-delete'])
            self.assertEqual(len(failure.__cause__.exceptions),2)
            if label in ('boot','bootstatus'):
                self.assertEqual(count,0);self.assertNotIn('owned-state',events);self.assertNotIn('actual-build-for-testing',events)
            if label!='actual-ui-test':self.assertNotIn('actual-ui-test',events)
        failure=RuntimeError('owned-artifact-proof')
        events,_,_,_=self.exercise(artifact_failure=failure,cleanup_errors=True)
        self.assertNotIn('actual-shared-trust-link-proof',events);self.assertNotIn('actual-ui-test',events)
        self.assertEqual(events[-2:],['cleanup-shutdown','cleanup-delete'])
