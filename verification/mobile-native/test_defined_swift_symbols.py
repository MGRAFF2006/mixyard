"""Controlled nm/demangler boundary tests, NOT compiler/SDK linkage evidence."""
import contextlib
import io
import json
import sys
import unittest
from unittest.mock import patch
import defined_swift_symbols as symbols

# Opaque synthetic substituted spellings: these are NOT claimed valid ABI
# manglings. The actual xcrun demangler is exercised only by hosted gates.
VERIFIER = '$s18MobileControlTrust09CanonicalbE0O6verifySynthetic'
PARSER = '$s9SwiftASN13DERO5parseSynthetic'
VERIFY_NAME = 'static MobileControlTrust.CanonicalControlCertificate.verify(FoundationEssentials.Data) throws -> ()'
PARSE_NAME = 'static SwiftASN1.DER.parse([Swift.UInt8]) throws -> SwiftASN1.ASN1Node'
NM = ('/PRIVATE/archive.a(member.o):\n000000000badcafe T _'+VERIFIER+'\n000000000badcb00 T _'+PARSER+'\n').encode()


class FakeTools:
    def __init__(self, nm=NM, demangled=None):
        self.nm = nm
        self.demangled = demangled if demangled is not None else (VERIFY_NAME+'\n'+PARSE_NAME+'\n').encode()
        self.calls = []
    def __call__(self, args, data=b''):
        self.calls.append((args,data))
        if args[:3] == ['xcrun','nm','-g']:
            value=self.nm[args[3]] if isinstance(self.nm,dict) else self.nm
        else:
            assert args == ['xcrun','swift-demangle','--compact']
            value=self.demangled
        if isinstance(value,Exception):raise value
        return value


class DefinedCodeProof(unittest.TestCase):
    def inspect(self, tools, fails=False, paths=None, role='qualifier', required=('verifier','derParseBytes')):
        output=io.StringIO()
        with contextlib.redirect_stdout(output):
            if fails:
                with self.assertRaises(RuntimeError) as failure:
                    symbols.attest_defined(paths or ['/PRIVATE/qualifier'],role,required=required,run=tools)
                self.assertNotIn('/PRIVATE',str(failure.exception))
            else:
                symbols.attest_defined(paths or ['/PRIVATE/qualifier'],role,required=required,run=tools)
        text=output.getvalue()
        self.assertEqual(len(text.splitlines()),1)
        self.assertNotIn('/PRIVATE',text);self.assertNotIn('badcafe',text);self.assertNotIn('$s',text)
        return json.loads(text.removeprefix('Mobile defined-code evidence: '))

    def test_semantic_identity_survives_substituted_raw_spelling(self):
        self.assertNotIn(b'CanonicalControlCertificate',NM)
        tools=FakeTools();receipt=self.inspect(tools)
        self.assertEqual(receipt['status'],'ok')
        self.assertTrue(all(receipt['functions'].values()))
        self.assertEqual(tools.calls[1][1],(VERIFIER+'\n'+PARSER+'\n').encode())
        self.assertEqual(receipt['counts']['demangled'],2)

    def test_undefined_data_and_plain_text_cannot_supply_defined_functions(self):
        for kind in ['U','D','t']:
            nm=('                 '+kind+' _'+VERIFIER+'\n000000000badcb00 T _'+PARSER+'\n').encode()
            # Defined non-text symbols still have a value in real nm output.
            if kind!='U':nm=('000000000badcafe '+kind+' _'+VERIFIER+'\n000000000badcb00 T _'+PARSER+'\n').encode()
            with self.subTest(kind=kind):
                tools=FakeTools(nm, (PARSE_NAME+'\n').encode())
                receipt=self.inspect(tools,fails=True)
                self.assertFalse(receipt['functions']['verifier'])
                self.assertNotIn(VERIFIER.encode(),tools.calls[-1][1])
        for nm in [(VERIFY_NAME+'\n').encode(), ('T _'+VERIFIER+'\n').encode(),
                   ('00000 T '+VERIFY_NAME+'\n').encode(), b'00000 ? _unknown\n']:
            with self.subTest(nm_shape=True):self.inspect(FakeTools(nm),fails=True)

    def test_wrong_namespace_thunk_overload_and_missing_function_reject(self):
        candidates=[VERIFY_NAME.replace('MobileControlTrust.','OtherModule.'),
                    'function signature specialization of '+VERIFY_NAME,
                    VERIFY_NAME.replace('.verify(','.verifyCanonical(')]
        for wrong in candidates:
            receipt=self.inspect(FakeTools(demangled=(wrong+'\n'+PARSE_NAME+'\n').encode()),fails=True)
            self.assertFalse(receipt['functions']['verifier'])
        wrong=PARSE_NAME.replace('[Swift.UInt8]','Swift.ArraySlice<Swift.UInt8>')
        receipt=self.inspect(FakeTools(demangled=(VERIFY_NAME+'\n'+wrong+'\n').encode()),fails=True)
        self.assertFalse(receipt['functions']['derParseBytes'])
        only=('00000 T _'+VERIFIER+'\n').encode()
        self.inspect(FakeTools(only,(VERIFY_NAME+'\n').encode()),fails=True)

    def test_partial_unknown_empty_or_failed_demangling_never_passes(self):
        for output in [b'',(VERIFY_NAME+'\n').encode(),
                       (VERIFY_NAME+'\n'+PARSE_NAME+'\nEXTRA\n').encode(),
                       (VERIFIER+'\n'+PARSER+'\n').encode(),b'\xff']:
            with self.subTest(output_length=len(output)):
                receipt=self.inspect(FakeTools(demangled=output),fails=True)
                self.assertIn(receipt['status'],('incomplete','decode-error'))
        for status in ['timeout','exit','os-error','unexpected-stderr','stop-incomplete']:
            receipt=self.inspect(FakeTools(demangled=symbols.ToolFailure(status,7)),fails=True)
            self.assertEqual(receipt['tool'],'demangle');self.assertEqual(receipt['status'],status)
        receipt=self.inspect(FakeTools(nm=symbols.ToolFailure('exit',1)),fails=True)
        self.assertEqual(receipt['tool'],'nm')

    def test_caps_fail_with_only_bounded_role_witness(self):
        for name,limit,cap in [('MAX_OUTPUT',16,'outputBytes'),('MAX_INPUT',16,'inputBytes'),('MAX_SYMBOLS',1,'symbols'),('MAX_LINES',1,'symbols')]:
            with self.subTest(cap=name),patch.object(symbols,name,limit):
                receipt=self.inspect(FakeTools(),fails=True)
                self.assertTrue(receipt['caps'][cap])
        receipt=self.inspect(FakeTools(),fails=True,paths=['/PRIVATE/a']*5)
        self.assertTrue(receipt['caps']['artifacts'])

    def test_app_stub_and_debug_code_are_checked_without_early_success(self):
        tools=FakeTools(nm={'/PRIVATE/app':b'                 U _external\n','/PRIVATE/debug':NM})
        receipt=self.inspect(tools,paths=['/PRIVATE/app','/PRIVATE/debug'],role='app',required=('verifier',))
        self.assertEqual(receipt['counts']['artifacts'],2)
        tools=FakeTools(nm={'/PRIVATE/app':NM,'/PRIVATE/debug':symbols.ToolFailure('exit',1)})
        receipt=self.inspect(tools,fails=True,paths=['/PRIVATE/app','/PRIVATE/debug'],role='app',required=('verifier',))
        self.assertEqual(receipt['status'],'exit')
        self.assertTrue(receipt['functions']['verifier']) # Earlier match cannot waive later incomplete inspection.


class BoundedOwnedPythonTool(unittest.TestCase):
    def command(self, code):return [sys.executable,'-B','-c',code]
    def test_concurrent_input_and_complete_output(self):
        data=b'bounded-input'*12000
        result=symbols.capture(self.command('import sys;sys.stdout.buffer.write(sys.stdin.buffer.read())'),data)
        self.assertEqual(result,data)
    def test_output_cap_stderr_exit_and_timeout_are_fixed_failures(self):
        with patch.object(symbols,'MAX_OUTPUT',128):
            with self.assertRaises(symbols.ToolFailure) as failure:
                symbols.capture(self.command('import sys;sys.stdout.write("x"*10000)'))
        self.assertEqual(failure.exception.cap,'outputBytes')
        for code,status in [('import sys;sys.stderr.write("PRIVATE_RAW_TOOL_OUTPUT")','unexpected-stderr'),('raise SystemExit(7)','exit')]:
            with self.assertRaises(symbols.ToolFailure) as failure:symbols.capture(self.command(code))
            self.assertEqual(failure.exception.status,status)
            self.assertNotIn('PRIVATE_RAW',str(failure.exception))
        with patch.object(symbols,'TOOL_SECONDS',0.05):
            with self.assertRaises(symbols.ToolFailure) as failure:
                symbols.capture(self.command('import time;time.sleep(10)'))
        self.assertEqual(failure.exception.status,'timeout')
