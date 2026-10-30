"""Failure-path-only observation. No result from this module admits a destination."""
import json
import os
import re
import selectors
import subprocess
import time

BUDGET_SECONDS = 3.0
STOP_RESERVE = 0.6
MAX_STDOUT = 256 * 1024
MAX_STDERR = 16 * 1024
MAX_DEVICE_ENTRIES = 256
MAX_SOURCE_BYTES = 256 * 1024
MAX_SOURCE_LINES = 1024
SIMCTL = ('xcrun','simctl','list','devices','--json')
UUID = re.compile(r'[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}')
PLACEHOLDERS = frozenset({
    'dvtdevice-DVTiPhonePlaceholder-iphoneos:placeholder',
    'dvtdevice-DVTiOSDeviceSimulatorPlaceholder-iphonesimulator:placeholder',
})
_HELD_CHILD = None # At most one unreaped OWN child; never discarded as completion.


def unknown_catalogue(reason, records=None, overflow=False):
    return {'status':'UNKNOWN','reason':reason,'completeSourceScan':False,
            'recordOverflow':overflow,'records':records or []}


def classify_catalogue(text, owned):
    if len(text)>MAX_SOURCE_BYTES or len(text.encode('utf-8'))>MAX_SOURCE_BYTES:
        return unknown_catalogue('source-byte-limit',overflow=True)
    records=[];section='unknown'
    for index,line in enumerate(text.splitlines(),1):
        if index>MAX_SOURCE_LINES:
            return unknown_catalogue('source-line-limit',records,True)
        if 'Available destinations for' in line:section='available'
        if 'Ineligible destinations for' in line:section='ineligible'
        remainder=re.sub(r'\{([^{}]+)\}', '', line)
        if '{' in remainder or '}' in remainder:
            return unknown_catalogue('source-brace-format',records)
        for match in re.finditer(r'\{([^{}]+)\}',line):
            if len(records)==8:
                return unknown_catalogue('source-record-limit',records,True)
            entry=match[1]
            if len(entry)>4096 or entry.count(',')>=32:
                return unknown_catalogue('source-field-limit',records,True)
            pairs=[part.strip().split(':',1) for part in entry.split(',') if ':' in part]
            fields={key.strip():value.strip() for key,value in pairs}
            identity=fields.get('id')
            kind='missing' if not identity else 'exact-owned' if identity==owned else 'known-placeholder-marker' if identity in PLACEHOLDERS else 'uuid-other' if UUID.fullmatch(identity) else 'other'
            records.append({'section':section,'idKind':kind,
                'platform':fields.get('platform') if fields.get('platform') in ('iOS Simulator','iOS','macOS') else 'other',
                'arch':fields.get('arch') if fields.get('arch') in ('arm64','x86_64') else 'missing-or-other',
                'archPresent':'arch' in fields,'errorPresent':'error' in fields,
                'duplicateFields':len(fields)!=len(pairs)})
    return {'status':'OBSERVED','reason':'same-line-brace-scan','completeSourceScan':True,
            'recordOverflow':False,'records':records}


def unknown_state(reason):
    return {'status':'UNKNOWN','reason':reason,'exactOwnedCount':'UNKNOWN','matchCountCapped':False,
            'runtimeGroup':'UNKNOWN','deviceType':'UNKNOWN','state':'UNKNOWN',
            'isAvailable':'UNKNOWN','availabilityErrorPresent':'UNKNOWN'}


def inspect_device_json(data, owned, runtime, device_type):
    def object_pairs(pairs):
        value={}
        for key,item in pairs:
            if key in value:raise ValueError()
            value[key]=item
        return value
    def invalid_constant(_value):raise ValueError()
    if len(data)>MAX_STDOUT:return unknown_state('stdout-limit')
    try:
        value=json.loads(data.decode('utf-8',errors='strict'),object_pairs_hook=object_pairs,parse_constant=invalid_constant)
    except (ValueError,UnicodeError,RecursionError):return unknown_state('json-invalid')
    if not isinstance(value,dict) or set(value)!={'devices'} or not isinstance(value['devices'],dict):
        return unknown_state('schema-invalid')
    groups=value['devices']
    if len(groups)>MAX_DEVICE_ENTRIES:return unknown_state('entry-limit')
    entries=0;matches=[];seen=set();other_duplicate=False
    for group,devices in groups.items():
        if not isinstance(group,str) or not group or not isinstance(devices,list):return unknown_state('schema-invalid')
        entries+=len(devices)
        if entries>MAX_DEVICE_ENTRIES:return unknown_state('entry-limit')
        for device in devices:
            if not isinstance(device,dict) or not isinstance(device.get('udid'),str) or not UUID.fullmatch(device['udid']):
                return unknown_state('schema-invalid')
            for field in ('state','deviceTypeIdentifier'):
                if field in device and not isinstance(device[field],str):return unknown_state('schema-invalid')
            if 'isAvailable' in device and type(device['isAvailable']) is not bool:return unknown_state('schema-invalid')
            if device['udid'] in seen and device['udid']!=owned:other_duplicate=True
            seen.add(device['udid'])
            if device['udid']==owned:matches.append((group,device))
    if other_duplicate:return unknown_state('duplicate-device-id')
    if len(matches)!=1:
        result=unknown_state('owned-absent' if not matches else 'owned-duplicate')
        result.update(exactOwnedCount=min(len(matches),2),matchCountCapped=len(matches)>2)
        return result
    group,device=matches[0]
    state=device.get('state')
    known_state=state if state in ('Shutdown','Booted','Booting','Shutting Down','Creating') else 'UNKNOWN'
    type_match='UNKNOWN' if not device.get('deviceTypeIdentifier') else 'MATCH' if device['deviceTypeIdentifier']==device_type else 'MISMATCH'
    available=device.get('isAvailable','UNKNOWN')
    reason='ok' if known_state!='UNKNOWN' and type_match!='UNKNOWN' and type(available) is bool else 'owned-fields-unknown'
    return {'status':'OBSERVED' if reason=='ok' else 'UNKNOWN','reason':reason,
            'exactOwnedCount':1,'matchCountCapped':False,
            'runtimeGroup':'MATCH' if group==runtime else 'MISMATCH','deviceType':type_match,
            'state':known_state,'isAvailable':available,'availabilityErrorPresent':'availabilityError' in device}


def read_devices(deadline, command=SIMCTL):
    """One own child; IO and stop/reap share the caller's SAME total budget."""
    global _HELD_CHILD
    receipt={'status':'UNKNOWN','reason':'not-started','toolValue':0,
             'directChildReaped':False,'ownedChildRetained':False,'pipesEof':False,
             'stdoutBytes':0,'stderrBytes':0}
    if command==SIMCTL and (os.environ.get('GITHUB_ACTIONS')!='true' or os.environ.get('RUNNER_OS')!='macOS'):
        receipt['reason']='hosted-mac-required';return None,receipt
    if _HELD_CHILD is not None:
        receipt.update(reason='prior-owned-child-unreaped',ownedChildRetained=True)
        return None,receipt
    process=None;selector=selectors.DefaultSelector()
    buffers={'stdout':bytearray(),'stderr':bytearray()};eof=set()
    try:
        io_deadline=deadline-STOP_RESERVE
        if time.monotonic()>=io_deadline:
            receipt['reason']='budget-exhausted';return None,receipt
        process=subprocess.Popen(command,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        for label,stream in [('stdout',process.stdout),('stderr',process.stderr)]:
            os.set_blocking(stream.fileno(),False);selector.register(stream,selectors.EVENT_READ,label)
        receipt['reason']='timeout'
        while selector.get_map():
            remaining=io_deadline-time.monotonic()
            if remaining<=0:break
            for key,_ in selector.select(remaining):
                limit=MAX_STDOUT if key.data=='stdout' else MAX_STDERR
                chunk=os.read(key.fd,min(65536,limit-len(buffers[key.data])+1))
                if not chunk:
                    eof.add(key.data);selector.unregister(key.fileobj);key.fileobj.close()
                else:
                    buffers[key.data].extend(chunk)
                    if len(buffers[key.data])>limit:
                        receipt['reason']=key.data+'-limit';return None,receipt
        if selector.get_map():return None,receipt
        try:code=process.wait(timeout=max(0,io_deadline-time.monotonic()))
        except subprocess.TimeoutExpired:return None,receipt
        receipt['toolValue']=code
        if code!=0:receipt['reason']='tool-exit'
        elif buffers['stderr']:receipt['reason']='stderr-present'
        else:
            receipt.update(status='COMPLETE',reason='ok')
            return bytes(buffers['stdout']),receipt
        return None,receipt
    except OSError as error:
        receipt.update(reason='os-error',toolValue=error.errno or 0);return None,receipt
    except BaseException:
        receipt['reason']='capture-internal';return None,receipt
    finally:
        selector.close()
        if process is not None:
            try:
                if process.poll() is None:
                    process.kill() # ONLY this direct child, never a service/group.
                    process.wait(timeout=max(0,deadline-time.monotonic()))
                receipt['directChildReaped']=process.returncode is not None
                if process.returncode is not None:receipt['toolValue']=process.returncode
            except (OSError,subprocess.TimeoutExpired):
                receipt.update(status='UNKNOWN',reason='stop-incomplete')
                _HELD_CHILD=process
                receipt['ownedChildRetained']=True
            finally:
                for stream in (process.stdout,process.stderr):
                    if not stream.closed:stream.close()
        receipt['pipesEof']=eof=={'stdout','stderr'}
        receipt['stdoutBytes']=len(buffers['stdout']);receipt['stderrBytes']=len(buffers['stderr'])
        if not receipt['directChildReaped'] and process is not None:
            receipt.update(status='UNKNOWN',reason='stop-incomplete',ownedChildRetained=True)
            _HELD_CHILD=process


def observe_rejection(text, owned, runtime, device_type):
    deadline=time.monotonic()+BUDGET_SECONDS
    try:catalogue=classify_catalogue(text,owned)
    except BaseException:catalogue=unknown_catalogue('internal')
    data,capture=read_devices(deadline)
    state=unknown_state('incomplete-capture' if capture['status']=='COMPLETE' else capture['reason'])
    if data is not None and capture['status']=='COMPLETE' and capture['directChildReaped'] and capture['pipesEof']:
        try:state=inspect_device_json(data,owned,runtime,device_type)
        except BaseException:state=unknown_state('internal')
    if time.monotonic()>deadline:state=unknown_state('budget-exhausted')
    print('Owned simulator rejected-admission observation: '+json.dumps({
        'phase':'rejected-destination-admission','observationOnly':True,
        'catalogue':catalogue,'capture':capture,'ownedState':state}),flush=True)
