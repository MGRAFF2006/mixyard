"""Defined-code proof from nm symbol types and public demangled identities.

Output is fixed-role metadata only: never raw symbols, addresses or tool paths.
"""
import json
import os
import re
import selectors
import subprocess
import time

MAX_OUTPUT = 8 * 1024 * 1024
MAX_ERROR = 64 * 1024
MAX_INPUT = 4 * 1024 * 1024
MAX_SYMBOLS = 32768
MAX_LINES = 131072
TOOL_SECONDS = 20
FUNCTIONS = {
    'verifier': frozenset({
        'static MobileControlTrust.CanonicalControlCertificate.verify(Foundation.Data) throws -> ()',
        'static MobileControlTrust.CanonicalControlCertificate.verify(FoundationEssentials.Data) throws -> ()',
    }),
    'derParseBytes': frozenset({
        'static SwiftASN1.DER.parse([Swift.UInt8]) throws -> SwiftASN1.ASN1Node',
        'static SwiftASN1.DER.parse(Swift.Array<Swift.UInt8>) throws -> SwiftASN1.ASN1Node',
    }),
}


class ToolFailure(Exception):
    def __init__(self, status, value=0, cap=None, shape=None):
        super().__init__('Defined-code tool did not produce complete admissible evidence')
        self.status, self.value, self.cap, self.shape = status, value, cap, shape


def capture(args, data=b''):
    """Drain capped stdout/stderr and demangler stdin concurrently, without files."""
    if len(data) > MAX_INPUT:
        raise ToolFailure('cap', cap='inputBytes')
    process = None
    selector = selectors.DefaultSelector()
    buffers = {'stdout':bytearray(), 'stderr':bytearray()}
    written = 0
    deadline = time.monotonic() + TOOL_SECONDS
    try:
        process = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        for label, stream in [('stdout',process.stdout),('stderr',process.stderr)]:
            os.set_blocking(stream.fileno(), False)
            selector.register(stream, selectors.EVENT_READ, label)
        if data:
            os.set_blocking(process.stdin.fileno(), False)
            selector.register(process.stdin, selectors.EVENT_WRITE, 'stdin')
        else:
            process.stdin.close()
        while selector.get_map():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise ToolFailure('timeout')
            for key, _ in selector.select(remaining):
                if key.data == 'stdin':
                    try:
                        written += os.write(key.fd, data[written:written+65536])
                    except BrokenPipeError:
                        raise ToolFailure('incomplete') from None
                    if written == len(data):
                        selector.unregister(key.fileobj); key.fileobj.close()
                else:
                    limit = MAX_OUTPUT if key.data == 'stdout' else MAX_ERROR
                    chunk = os.read(key.fd, min(65536, limit-len(buffers[key.data])+1))
                    if chunk:
                        buffers[key.data].extend(chunk)
                        if len(buffers[key.data]) > limit:
                            raise ToolFailure('cap', cap='outputBytes')
                    else:
                        selector.unregister(key.fileobj); key.fileobj.close()
        try:
            code = process.wait(timeout=max(0,deadline-time.monotonic()))
        except subprocess.TimeoutExpired:
            raise ToolFailure('timeout') from None
        if code != 0:
            raise ToolFailure('exit', code)
        if buffers['stderr']:
            raise ToolFailure('unexpected-stderr')
        if written != len(data):
            raise ToolFailure('incomplete')
        return bytes(buffers['stdout'])
    except OSError as error:
        raise ToolFailure('os-error', error.errno or 0) from None
    finally:
        selector.close()
        if process is not None:
            try:
                if process.poll() is None:
                    process.kill()
                    process.wait(timeout=2)
            except (OSError, subprocess.TimeoutExpired):
                raise ToolFailure('stop-incomplete') from None
            finally:
                for stream in (process.stdin,process.stdout,process.stderr):
                    if not stream.closed: stream.close()


def record_shape(line, index, why):
    # Diagnostic classification ONLY. No field below grants parser acceptance.
    tokens = line.split(None, 8)
    first = tokens[0] if tokens else ''
    address = 'HEX' if re.fullmatch(r'[0-9A-Fa-f]+', first) else 'DASH' if re.fullmatch(r'-+', first) else 'NONE'
    candidate = tokens[1] if address != 'NONE' and len(tokens)>1 else first
    # A fixed single-letter/question-mark alphabet is safe even for an as-yet
    # unsupported nm kind; never publish arbitrary token/symbol/header text.
    kind = candidate if len(candidate)==1 and candidate in 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz?' else 'UNKNOWN'
    return {'why':why,'lineIndex':min(index,MAX_LINES),
            'tokenCount':len(tokens),'tokenCountCapped':len(tokens)==9,
            'addressShape':address,'recognisedKind':kind,
            'swiftPrefix':any(token.startswith(('_$s','$s')) for token in tokens[:8]),
            'indirectTargetMarker':'(indirect for ' in line,
            'headerLike':line.endswith(':')}


def defined_swift_names(output, counts):
    try:
        lines = output.decode('utf-8', errors='strict').splitlines()
    except UnicodeError:
        raise ToolFailure('decode-error') from None
    if len(lines) > MAX_LINES:
        raise ToolFailure('cap', cap='symbols')
    names = []
    for index, line in enumerate(lines, 1):
        line = line.strip()
        if not line:
            continue
        # Archive/member and Mach-O architecture headers are not code evidence.
        if line.endswith(':') and '$s' not in line:
            continue
        # BSD nm emits an opaque name after its address/type prefix. Mach-O
        # S records are non-text data and may contain spaces (e.g. metadata).
        # Recognise only the observed addressed S form; never demangle it or
        # let it contribute to defined-function availability. Other malformed
        # shapes still fail through the unchanged parser below.
        if re.fullmatch(r'[0-9A-Fa-f]+\s+S\s+\S+(?:[ \t]+\S+)+', line):
            counts['nonTextMetadata'] += 1
            continue
        match = re.fullmatch(r'(?:(?P<address>[0-9A-Fa-f]+)\s+)?(?P<kind>[A-Za-z?])\s+(?P<name>\S+)', line)
        if match is None:
            raise ToolFailure('nm-shape', shape=record_shape(line,index,'unparsed-columns'))
        kind, name = match['kind'], match['name']
        if kind not in 'ABCDGINPRSTUVWabcdginprstuvw':
            raise ToolFailure('nm-shape', shape=record_shape(line,index,'unknown-kind'))
        if kind == 'U':
            counts['undefined'] += 1
            continue
        if kind != 'T':
            continue
        if match['address'] is None:
            raise ToolFailure('nm-shape', shape=record_shape(line,index,'missing-text-address'))
        counts['definedText'] += 1
        if name.startswith(('_$s','$s')):
            if len(name)>4096:
                raise ToolFailure('cap', cap='inputBytes')
            names.append(name[1:] if name.startswith('_$s') else name)
            counts['swiftDefined'] += 1
    if len(names)>MAX_SYMBOLS:
        raise ToolFailure('cap', cap='symbols')
    return names


def attest_defined(paths, role, required=('verifier','derParseBytes'), run=capture):
    if role not in ('trust-archive','qualifier','app') or not required or any(name not in FUNCTIONS for name in required):
        raise ValueError('Invalid fixed attestation role/function requirement')
    availability = dict.fromkeys(FUNCTIONS, False)
    counts = {'artifacts':0,'definedText':0,'undefined':0,'swiftDefined':0,'demangled':0,'nonTextMetadata':0}
    caps = dict.fromkeys(('artifacts','inputBytes','outputBytes','symbols'), False)
    tool, status, value = 'semantic', 'ok', 0
    shape = None
    try:
        if not paths or len(paths)>4:
            raise ToolFailure('cap', cap='artifacts')
        for path in paths:
            counts['artifacts'] += 1
            tool = 'nm'
            output = run(['xcrun','nm','-g',str(path)])
            if len(output)>MAX_OUTPUT:
                raise ToolFailure('cap', cap='outputBytes')
            names = defined_swift_names(output, counts)
            if not names:
                continue # App entry stub may defer to its actual debug dylib.
            data = ('\n'.join(names)+'\n').encode('ascii')
            if len(data)>MAX_INPUT:
                raise ToolFailure('cap', cap='inputBytes')
            tool = 'demangle'
            output = run(['xcrun','swift-demangle','--compact'], data)
            if len(output)>MAX_OUTPUT:
                raise ToolFailure('cap', cap='outputBytes')
            try:
                demangled = output.decode('utf-8',errors='strict').splitlines()
            except UnicodeError:
                raise ToolFailure('decode-error') from None
            if len(demangled)!=len(names) or any(not result or len(result)>4096 or '$s' in result for result in demangled):
                raise ToolFailure('incomplete')
            counts['demangled'] += len(demangled)
            for name, identities in FUNCTIONS.items():
                availability[name] |= any(result in identities for result in demangled)
        tool = 'semantic'
        if not all(availability[name] for name in required):
            raise ToolFailure('required-function-missing')
    except (ToolFailure, UnicodeError) as failure:
        if isinstance(failure, ToolFailure):
            status, value, shape = failure.status, failure.value, failure.shape
            if failure.cap: caps[failure.cap] = True
        else:
            status = 'decode-error'
    receipt = {'role':role,'functions':availability,'required':list(required),'tool':tool,
               'status':status,'value':value,'counts':counts,'caps':caps,'recordShape':shape}
    print('Mobile defined-code evidence: '+json.dumps(receipt,sort_keys=True),flush=True)
    if status != 'ok':
        raise RuntimeError('Defined Swift code attestation failed; see bounded role receipt') from None
    return availability
