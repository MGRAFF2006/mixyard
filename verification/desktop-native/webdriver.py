"""Owned Linux release webview and explicitly harness-started headless runtime."""
import base64
import http.client
import json
import math
import os
from pathlib import Path
import signal
import socket
import stat
import struct
import subprocess
import sys
import time
import traceback
import urllib.error
import urllib.request

from evidence import binary_info, disk_guard, inventory

ELEMENT = 'element-6066-11e4-a52e-4f735466cecf'


def wait_for(predicate, label, seconds=20):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(0.05)
    raise AssertionError(f'Timed out: {label}')


def xdotool(*args):
    return subprocess.run(['xdotool', *map(str, args)], check=True,
                          capture_output=True, text=True, timeout=10).stdout.strip()


class Driver:
    def __init__(self):
        self.session = None
        self.window_id = None
        self.input_attempts = []

    def request(self, method, path, body=None):
        request = urllib.request.Request(
            'http://127.0.0.1:4444' + path,
            data=json.dumps(body).encode() if body is not None else None,
            headers={'Content-Type': 'application/json'}, method=method)
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                value = json.load(response)['value']
        except urllib.error.HTTPError as error:
            raise RuntimeError(f'WebDriver HTTP {error.code}: {error.read().decode()}') from error
        if isinstance(value, dict) and 'error' in value:
            raise RuntimeError(str(value))
        return value

    def call(self, method, path, body=None):
        return self.request(method, f'/session/{self.session}{path}', body)

    def script(self, script):
        return self.call('POST', '/execute/sync', {'script': script, 'args': []})

    def click(self, selector):
        element = self.call('POST', '/element', {'using': 'css selector', 'value': selector})
        return self.call('POST', f'/element/{element[ELEMENT]}/click', {})

    def delete(self):
        if self.session:
            try:
                self.call('DELETE', '')
            except (OSError, http.client.HTTPException, RuntimeError) as error:
                print(f'Session cleanup after UI exit: {error}', flush=True)
            self.session = None


def window():
    result = subprocess.run(['xdotool', 'search', '--onlyvisible', '--name', '^Mixyard$'],
                            capture_output=True, text=True, timeout=10)
    return result.stdout.strip().splitlines()[0] if result.returncode == 0 else None


def exited(pid):
    path = Path(f'/proc/{pid}/stat')
    try:
        status = path.read_text()
    except (FileNotFoundError, ProcessLookupError):
        # Disappearance can fail during open (ENOENT) or an already-open read (ESRCH).
        return True
    return status.split(') ')[1].startswith('Z')


def stop(process):
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)


def headless_env():
    return {key: value for key, value in os.environ.items() if key not in {
        'DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS',
        'DBUS_SYSTEM_BUS_ADDRESS', 'PULSE_SERVER', 'PIPEWIRE_REMOTE'}}


def cli_request(cli, instance, command, ready=False):
    result = subprocess.run([str(cli), command, '--instance', instance],
                            env=headless_env(), capture_output=True, text=True, timeout=10)
    if ready and result.returncode != 0:
        return None
    assert result.returncode == 0, f'CLI {command}: {result.stderr}'
    assert not result.stderr, result.stderr
    reply = json.loads(result.stdout)
    assert reply['ipcVersion'] == 1 and reply['requestId'] and 'error' not in reply, reply
    return reply


def assert_unavailable(availability):
    assert availability['daemonReachable'] is True, availability
    assert availability['backendAvailable'] is False and availability['backend'] == 'unavailable', availability
    assert availability['nativeAudioProof'] == 'unverified', availability
    for revision in ['desiredRevision', 'audioRevision', 'appliedRevision']:
        assert revision in availability and availability[revision] is None, availability
    assert availability['capabilities'] == {'apply': False, 'mute': False, 'nativeActionDispatch': False}


def runtime_alive(child, endpoint, cli, instance):
    assert child.poll() is None, 'Harness runtime exited unexpectedly'
    # CLI success plus the kernel peer PID ties this endpoint to our exact child,
    # rather than to a sentinel or another runtime under the same OS user.
    with socket.socket(socket.AF_UNIX) as peer:
        peer.settimeout(5)
        peer.connect(str(endpoint))
        pid, uid, _ = struct.unpack('3i', peer.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
    assert pid == child.pid and uid == os.geteuid(), (pid, uid, child.pid)
    reply = cli_request(cli, instance, 'status')
    assert_unavailable(reply['result'])
    assert child.poll() is None
    return {'pid': pid, 'peer_uid': uid, 'cli_status': reply}


def shell_status(driver):
    return driver.call('POST', '/execute/async', {'script':
        "const done = arguments[arguments.length - 1]; window.__TAURI_INTERNALS__.invoke('shell_status').then(done, error => done({error: String(error)}));",
        'args': []})


def input_state(driver):
    return driver.script("""
        const input = document.querySelector('#runtime-instance');
        const active = document.activeElement;
        const rect = input?.getBoundingClientRect();
        return {documentFocused: document.hasFocus(),
          active: active ? {tag: active.tagName, id: active.id, value: active.value,
            text: active.textContent?.slice(0, 160)} : null,
          input: input ? {value: input.value, disabled: input.disabled,
            x: rect.x, y: rect.y, width: rect.width, height: rect.height} : null,
          viewport: {width: innerWidth, height: innerHeight},
          runtimeStatus: document.querySelector('#runtime-status')?.textContent,
          runtimeDetail: document.querySelector('#runtime-detail')?.textContent};
    """)


def native_focus(driver, selector, click=False):
    assert driver.window_id, 'Current UI native window must be known'
    xdotool('windowactivate', '--sync', driver.window_id)
    assert int(xdotool('getactivewindow')) == int(driver.window_id), 'Wrong native window active'
    wait_for(lambda: driver.script(f"const e = document.querySelector({json.dumps(selector)}); return e && !e.disabled"), 'enabled ' + selector)
    driver.script(f"document.querySelector({json.dumps(selector)}).scrollIntoView({{block: 'center', behavior: 'instant'}});")
    if click:
        # W3C element click also gives GTK/WebKit keyboard focus after resizing.
        # Text entry below still comes only from trusted OS keyboard events.
        driver.click(selector)
    settled = driver.call('POST', '/execute/async', {'script': f"""
        const done = arguments[arguments.length - 1];
        const element = document.querySelector({json.dumps(selector)});
        element.focus({{preventScroll: true}});
        requestAnimationFrame(() => requestAnimationFrame(() => {{
          const rect = element.getBoundingClientRect();
          done(document.hasFocus() && document.activeElement === element && !element.disabled
            && rect.width > 0 && rect.height > 0 && rect.top >= 0 && rect.bottom <= innerHeight);
        }}));
    """, 'args': []})
    assert settled, f'Native focus did not settle for {selector}: {input_state(driver)!r}'


def native_button(driver, selector):
    native_focus(driver, selector)
    driver.script("""
        window.nativeActivation = null;
        document.activeElement.addEventListener('keydown', event => {
          if (event.key === 'Enter') window.nativeActivation = event.isTrusted;
        }, {once: true});
    """)
    xdotool('key', '--clearmodifiers', 'Return')
    # XTest command completion does not mean WebKit consumed its queued events.
    wait_for(lambda: driver.script('return window.nativeActivation') is True,
             'trusted native key activation', seconds=2)


def submit_connect(driver, instance):
    native_focus(driver, '#runtime-instance', click=True)
    attempt = {'requested_instance': instance, 'native_window': driver.window_id,
               'active_window_before': xdotool('getactivewindow'), 'before': input_state(driver)}
    driver.input_attempts.append(attempt)
    xdotool('key', '--clearmodifiers', 'ctrl+a')
    xdotool('type', '--clearmodifiers', '--delay', 10, instance)
    def entered():
        attempt['after'] = input_state(driver)
        return attempt['after']['input']['value'] == instance
    wait_for(entered, f'native runtime instance entry {instance!r}', seconds=2)
    attempt['active_window_after'] = xdotool('getactivewindow')
    xdotool('key', 'Tab')
    wait_for(lambda: driver.script("return document.activeElement.id") == 'runtime-connect',
             'native Tab to runtime connect', seconds=2)
    native_button(driver, '#runtime-connect')


def connected_status(driver, instance):
    status = wait_for(lambda: (value if (value := shell_status(driver)).get('connected') else None),
                      'connected real runtime snapshot')
    assert status['schemaVersion'] == 1 and status['runtime']['instance'] == instance
    assert status['runtime']['phase'] == 'connected'
    event = status['runtime']['event']
    assert event['event'] == 'snapshot' and event['sequence'] == 1
    assert event['resync'] is True and event['historyAvailable'] is False
    assert event['lifecycle'] == 'running'
    assert_unavailable(event['availability'])
    wait_for(lambda: driver.script("return document.querySelector('#runtime-status').textContent.toLowerCase().includes('connected · audio unavailable') && document.querySelector('#runtime-detail').textContent.includes('backendAvailable: false')"),
             'connected UI honestly reports unavailable audio backend')
    assert driver.script("return ['IPC v1', 'snapshot sequence 1', 'backendAvailable: false', 'audioRevision: null', 'appliedRevision: null', 'desiredRevision: null'].every(text => document.querySelector('#runtime-detail').textContent.includes(text))"), 'Versioned snapshot and null revisions must be visible'
    assert driver.script("return !document.querySelector('#runtime-status').matches('input, select, textarea, [contenteditable=true]')"), 'Connection status is read-only'
    assert driver.script("return [...document.querySelectorAll('.desktop-toolbar button')].some(e => e.disabled && e.textContent.includes('Audio mute unavailable'))"), 'Connected IPC cannot unlock live audio mute'
    return status


def connect(driver, instance):
    submit_connect(driver, instance)
    return connected_status(driver, instance)


def worker_names(pid):
    names = [path.read_text().strip() for path in Path(f'/proc/{pid}/task').glob('*/comm')]
    workers = [name for name in names if name.startswith('runtime-readon')]
    assert len(workers) == 1, f'Expected exactly one runtime worker, got {workers}'
    return workers


def disconnect(driver):
    started = time.monotonic()
    native_button(driver, '#runtime-disconnect')
    wait_for(lambda: shell_status(driver)['runtime']['phase'] == 'disconnected', 'prompt disconnect')
    elapsed = (time.monotonic() - started) * 1000
    assert elapsed < 1000, f'Disconnect blocked for {elapsed:.1f} ms'
    wait_for(lambda: driver.script("return document.querySelector('#runtime-connect') !== null"), 'disconnected UI button')
    return elapsed


def late_results(driver, ui_pid, child, instance, availability):
    # One small, owned native socket fixture. Its generated frames never count
    # as evidence from the real runtime used by the survival cases below.
    fixture_instance = f'native-{os.getpid()}-late'
    directory = Path(f'/tmp/mixyard-{os.geteuid()}-{fixture_instance}')
    directory.mkdir(mode=0o700)
    endpoint = directory / 'runtime.sock'
    evidence = []
    try:
        with socket.socket(socket.AF_UNIX) as listener:
            listener.bind(str(endpoint))
            endpoint.chmod(0o600)
            listener.listen(1)
            listener.settimeout(5)
            for late in ['snapshot', 'lifecycle']:
                submit_connect(driver, fixture_instance)
                peer, _ = listener.accept()
                with peer:
                    peer.settimeout(5)
                    peer_pid, uid, _ = struct.unpack('3i', peer.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
                    assert peer_pid == ui_pid and uid == os.geteuid()
                    request_bytes = b''
                    while b'\n' not in request_bytes:
                        chunk = peer.recv(1024)
                        assert chunk, 'Native client closed before subscription'
                        request_bytes += chunk
                        assert len(request_bytes) <= 8192
                    request = json.loads(request_bytes)
                    assert request['ipcVersion'] == 1 and request['command'] == 'events'
                    snapshot = {'event': 'snapshot', 'sequence': 1, 'resync': True,
                                'lifecycle': 'running', 'availability': availability, 'historyAvailable': False}

                    def send(event):
                        peer.sendall((json.dumps({'ipcVersion': 1, 'requestId': request['requestId'],
                                                 'result': event}) + '\n').encode())

                    if late == 'lifecycle':
                        send(snapshot)
                        connected_status(driver, fixture_instance)
                    else:
                        assert shell_status(driver)['runtime']['phase'] == 'connecting'
                    workers_before = worker_names(ui_pid)
                    disconnect_ms = disconnect(driver)
                    # Hold only our own headless child so a stale response cannot
                    # hide behind an immediately arriving fresh real snapshot.
                    os.kill(child.pid, signal.SIGSTOP)
                    try:
                        submit_connect(driver, instance)
                        delivery = 'written-to-old-socket; consumption unverified'
                        try:
                            send(snapshot if late == 'snapshot' else {
                                'event': 'lifecycle', 'sequence': 2,
                                'lifecycle': 'stopping', 'backendAvailable': False})
                        except (BrokenPipeError, ConnectionResetError):
                            delivery = 'old subscription already closed; no delivery claimed'
                            assert late == 'lifecycle', 'Delayed snapshot must be released on the open old peer'
                        samples = []
                        deadline = time.monotonic() + .35
                        while time.monotonic() < deadline:
                            current = shell_status(driver)
                            assert current['connected'] is False and current['runtime']['phase'] == 'connecting', current
                            assert current['runtime']['instance'] == instance and current['runtime']['event'] is None, current
                            samples.append({'phase': current['runtime']['phase'],
                                            'instance': current['runtime']['instance']})
                            worker_names(ui_pid)
                            time.sleep(.03)
                    finally:
                        os.kill(child.pid, signal.SIGCONT)
                    current = connected_status(driver, instance)
                    evidence.append({'action': f'old-generation-{late}-rejected',
                        'fixture': 'owned private same-UID Unix lifecycle socket; generated test frames only',
                        'old_instance': fixture_instance, 'new_instance': instance,
                        'old_reply': delivery, 'disconnect_ms': disconnect_ms,
                        'workers_before': workers_before, 'workers_after': worker_names(ui_pid),
                        'fresh_snapshot_held_samples': samples, 'real_runtime_shell_status': current})
                    disconnect(driver)
    finally:
        endpoint.unlink(missing_ok=True)
        directory.rmdir()
    return evidence


def close_confirmation(driver, accept):
    def dialog():
        try:
            text = driver.call('GET', '/alert/text')
            return text if 'Unsaved drafts' in text else None
        except RuntimeError:
            return None
    wait_for(dialog, 'shared unsaved draft close confirmation')
    driver.call('POST', '/alert/accept' if accept else '/alert/dismiss', {})


def main(binary, runtime, cli, web, output):
    if os.environ.get('MIXYARD_NATIVE_ISOLATED') != '1':
        raise SystemExit('Use run-linux.sh: only an owned Xvfb/session bus is allowed.')
    disk_guard()
    report = inventory(binary)
    report['runtime_binaries'] = {'runtime': binary_info(runtime), 'cli': binary_info(cli), 'web': binary_info(web)}
    report['boundary'] = 'Actual private headless lifecycle IPC and native Linux webview only; backend unavailable, no audio engine, physical tray/Dock, assistive or other-platform GUI proof.'
    report.update(status='failed', display='owned Xvfb 1280x960, Openbox, software rendering, no tray host',
                  steps=[], timing={})
    driver = Driver()
    processes = []
    pid = None
    runtime_child = None
    events_child = None
    instance = f'native-{os.getpid()}'
    endpoint = Path(f'/tmp/mixyard-{os.geteuid()}-{instance}/runtime.sock')
    directory_identity = None
    report['runtime-survival'] = {'status': 'not-completed', 'audio_backend': 'unavailable'}
    report['runtime'] = {'instance': instance, 'endpoint': str(endpoint),
                         'startup_owner': 'test harness, never app', 'headless': True}
    try:
        # The production endpoint ignores XDG_RUNTIME_DIR. Refuse any existing
        # namespace; never unlink or reuse another process's endpoint.
        assert not endpoint.parent.exists() and not endpoint.parent.is_symlink(), endpoint
        with (output / 'runtime.log').open('w') as log:
            runtime_child = subprocess.Popen([str(runtime), 'serve', '--instance', instance],
                env=headless_env(), stdout=log, stderr=log, start_new_session=True)
        processes.append(runtime_child)
        report['runtime']['pid'] = runtime_child.pid

        def runtime_ready():
            assert runtime_child.poll() is None, 'Runtime exited before readiness'
            return cli_request(cli, instance, 'status', ready=True)
        wait_for(runtime_ready, 'explicit harness runtime readiness')
        directory_identity = endpoint.parent.stat()
        assert stat.S_IMODE(directory_identity.st_mode) == 0o700
        assert directory_identity.st_uid == os.geteuid()
        assert stat.S_ISSOCK(endpoint.stat().st_mode) and stat.S_IMODE(endpoint.stat().st_mode) == 0o600
        report['runtime']['ready'] = runtime_alive(runtime_child, endpoint, cli, instance)
        events_child = subprocess.Popen([str(cli), 'events', '--instance', instance],
            env=headless_env(), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, start_new_session=True)
        processes.append(events_child)
        # Launch a separate production console service with disposable storage.
        # The desktop bridge remains fixed to a numeric loopback port inherited at launch.
        with socket.socket() as reservation:
            reservation.bind(('127.0.0.1', 0))
            port = reservation.getsockname()[1]
        os.environ['MIXYARD_WEB_PORT'] = str(port)
        store = output / 'owned-profile-store'
        assert not store.exists(), 'Never reuse another run/store'
        with (output / 'web-service.log').open('w') as log:
            web_child = subprocess.Popen([str(web), '--port', str(port), '--instance', instance,
                '--store', str(store), '--assets', str(Path(__file__).resolve().parents[2] / 'apps/desktop/dist')],
                env=headless_env(), stdout=log, stderr=log, start_new_session=True)
        processes.append(web_child)
        def web_ready():
            assert web_child.poll() is None, 'Production console service exited'
            try:
                with urllib.request.urlopen(f'http://127.0.0.1:{port}/api/session', timeout=2) as reply:
                    session = json.load(reply)
                    return session['apiVersion'] == 1 and session['audioAvailable'] is False
            except (urllib.error.URLError, ConnectionError):
                return False
        wait_for(web_ready, 'production console service ready')
        report['console-service'] = {'pid': web_child.pid, 'port': port, 'store': str(store),
            'owner': 'harness; independent of UI', 'audio_available': False}
        with (output / 'wm.log').open('w') as log:
            processes.append(subprocess.Popen(['openbox'], stdout=log, stderr=log, start_new_session=True))
        with (output / 'tauri-driver.log').open('w') as log:
            processes.append(subprocess.Popen(['tauri-driver'], stdout=log, stderr=log, start_new_session=True))

        def ready():
            try:
                driver.request('GET', '/status')
                return True
            except (urllib.error.URLError, http.client.HTTPException):
                return False
        wait_for(ready, 'tauri-driver ready')

        for action in ['native-close', 'close-window', 'quit-ui', 'runtime-shutdown']:
            session = driver.request('POST', '/session', {'capabilities': {'alwaysMatch': {
                'browserName': 'wry', 'tauri:options': {'application': str(binary)}}}})
            driver.session = session['sessionId']
            report['steps'].append({'action': 'session-created', 'case': action,
                                    'url': driver.call('GET', '/url')})
            wait_for(lambda: driver.script(
                "return document.querySelector('#runtime-instance') !== null"),
                'runtime connection form')
            wid = wait_for(window, 'visible Mixyard native window')
            driver.window_id = wid
            pid = int(xdotool('getwindowpid', wid))
            xdotool('windowactivate', '--sync', wid)
            report['steps'].append({'action': 'launch/relaunch', 'case': action, 'pid': pid,
                                    'url': driver.call('GET', '/url'),
                                    'controls_locked': driver.script(
                                        "return [...document.querySelectorAll('.desktop-toolbar button')].some(e => e.disabled && e.textContent.includes('Audio mute unavailable'))")})
            assert report['steps'][-1]['controls_locked'], 'Live mute must remain unavailable'
            assert driver.script("return '__TAURI_INTERNALS__' in window"), 'Must execute the Tauri webview'
            status = shell_status(driver)
            assert status['connected'] is False and status['platform'] == 'linux'
            assert status['trayCreated'] is False, 'No-tray fallback must be explicit in this harness'
            assert status['runtime']['phase'] == 'disconnected' and status['runtime']['event'] is None
            assert driver.script("return document.querySelector('#runtime-instance').value") == 'default'
            report['steps'][-1]['shell_status'] = status

            if action == 'native-close':
                # The actual shared console, not the removed prototype fixture mixer.
                wait_for(lambda: driver.script("return [...document.querySelectorAll('.console .secondary')].some(e => e.textContent.includes('Store revision')) && [...document.querySelectorAll('.console .message')].every(e => !e.textContent.includes('Action failed')) && [...document.querySelectorAll('.console button')].some(e => e.textContent === 'Load simulated example' && !e.disabled)"), 'shared console ready')
                driver.script("document.querySelector('.console button').blur(); [...document.querySelectorAll('.console button')].find(e => e.textContent === 'Load simulated example').focus();")
                xdotool('key', 'Return')
                wait_for(lambda: driver.script("return document.querySelector('.simulated') !== null && document.querySelector('.matrix input[type=range]') !== null"), 'explicit shared example')
                driver.script("""
                    const range = document.querySelector('.matrix input[type=range]');
                    range.scrollIntoView({block: 'center'}); range.focus();
                    window.nativeSamples = [];
                    range.addEventListener('keydown', event => {
                      if (!['ArrowLeft', 'ArrowRight'].includes(event.key)) return;
                      const start = performance.now(), before = range.value;
                      requestAnimationFrame(() => requestAnimationFrame(() => {
                        window.nativeSamples.push({ms: performance.now() - start,
                          trusted: event.isTrusted, before, after: range.value,
                          output: range.parentElement.querySelector('output').textContent,
                          focus: getComputedStyle(range).outlineWidth});
                      }));
                    });
                """)
                for load in ['normal-no-added-load', 'one-owned-busy-CPU-worker']:
                    worker = None
                    if load != 'normal-no-added-load':
                        worker = subprocess.Popen([sys.executable, '-c',
                            'import hashlib\nwhile True: hashlib.sha256(b"x" * 65536).digest()'], start_new_session=True)
                        processes.append(worker)
                    try:
                        driver.script('window.nativeSamples = [];')
                        for index in range(60):
                            xdotool('key', 'Left' if index % 2 == 0 else 'Right')
                            wait_for(lambda: driver.script('return window.nativeSamples.length') == index + 1,
                                     'trusted key rendered')
                        samples = driver.script('return window.nativeSamples')
                        assert all(s['trusted'] and s['before'] != s['after'] for s in samples)
                        assert all(s['output'] == f"{float(s['after']) * 100:.0f}%"
                                   for s in samples), 'Vue output must reflect each real input'
                        assert all(float(s['focus'].removesuffix('px')) >= 2 for s in samples)
                        p95 = sorted(s['ms'] for s in samples)[math.ceil(len(samples) * .95) - 1]
                        report['timing'][load] = {'samples': samples, 'p95_ms': p95,
                            'status': 'passed' if p95 <= 100 else 'failed',
                            'measurement': 'trusted OS keydown in actual release WebKit webview to second rAF; excludes pre-dispatch OS queue; not physical presentation or audio latency',
                            'load_average': os.getloadavg()}
                    finally:
                        if worker:
                            stop(worker)
                (output / 'shared-console-webview.png').write_bytes(base64.b64decode(driver.call('GET', '/screenshot')))
                native_button(driver, '#compact-window')
                wait_for(lambda: driver.script('return innerWidth') <= 480, 'compact native command resizes same window')
                assert driver.script('return document.documentElement.scrollWidth <= innerWidth'), 'Compact overflow'
                (output / 'compact-webview.png').write_bytes(base64.b64decode(driver.call('GET', '/screenshot')))
                # Native titlebar close must allow cancelling draft loss before exiting.
                xdotool('key', 'Alt+F4')
                close_confirmation(driver, accept=False)
                assert not exited(pid), 'Cancel must retain the same UI and unsaved draft'
                report['steps'].append({'action': 'unsaved-titlebar-close-cancelled', 'pid': pid})
                # Both native profile shortcuts and the full UI use the same console session.
                native_button(driver, '#open-profiles')
                wait_for(lambda: driver.script("return document.querySelector('.console nav button[aria-current=page]').textContent === 'Profiles'"), 'shared profiles navigation')
                native_button(driver, '#compact-window')
                wait_for(lambda: driver.script('return innerWidth') >= 1000, 'restore full window')
                native_button(driver, '.console .draft-bar button.primary')
                wait_for(lambda: driver.script("return document.querySelector('.draft-bar strong').textContent === 'Draft matches stored copy'"), 'native bridge persists shared draft')
                native_focus(driver, '#profile-name', click=True)
                xdotool('type', '--clearmodifiers', '--delay', 10, 'Native shared profile')
                wait_for(lambda: driver.script("return document.querySelector('#profile-name').value") == 'Native shared profile', 'native profile name input')
                native_button(driver, '.console .profile-editor button.primary')
                wait_for(lambda: driver.script("return [...document.querySelectorAll('#profile-select option')].some(e => e.textContent.includes('Native shared profile'))"), 'profile persisted by shared console')
                # A separately read production-store acknowledgment proves this was not
                # an in-webview fixture/model edit. Do not write or publish the CSRF token.
                with urllib.request.urlopen(f'http://127.0.0.1:{port}/api/session', timeout=2) as response:
                    token = json.load(response)['csrfToken']
                request = urllib.request.Request(f'http://127.0.0.1:{port}/api/profiles/read',
                    data=b'{}', headers={'Content-Type': 'application/json', 'X-Mixyard-Token': token})
                with urllib.request.urlopen(request, timeout=5) as response:
                    stored = json.load(response)
                assert stored['storeRevision'] == 2 and len(stored['profiles']) == 1
                assert stored['profiles'][0]['name'] == 'Native shared profile'
                assert stored['draft']['sources'][0]['selector']['deviceUid'].startswith('example:')
                report['steps'].append({'action': 'shared-console-native-store-writes',
                    'storeRevision': stored['storeRevision'], 'profiles': len(stored['profiles']),
                    'evidence': 'real separate production service/store; explicit simulated routing intent, no audio'})


            if action == 'native-close':
                report['steps'].extend(late_results(driver, pid, runtime_child, instance,
                    report['runtime']['ready']['cli_status']['result']))
            connected = connect(driver, instance)
            report['steps'].append({'action': 'native-keyboard-connect', 'case': action,
                                    'shell_status': connected})
            if action == 'native-close':
                disconnect(driver)
                worker_names(pid)
                report['steps'].append({'action': 'disconnect-runtime-survives',
                    **runtime_alive(runtime_child, endpoint, cli, instance)})
                report['steps'].append({'action': 'native-keyboard-reconnect', 'shell_status': connect(driver, instance)})
                xdotool('key', 'Alt+F4')
                close_confirmation(driver, accept=True)
            else:
                if action == 'runtime-shutdown':
                    shutdown = cli_request(cli, instance, 'shutdown')
                    assert shutdown['result']['shutdown'] == 'requested'
                    assert runtime_child.wait(timeout=10) == 0, 'Explicit shutdown must reap child successfully'
                    stdout, stderr = events_child.communicate(timeout=10)
                    assert events_child.returncode == 0 and not stderr, stderr
                    events = [json.loads(line) for line in stdout.splitlines()]
                    assert len(events) == 2 and all(event['ipcVersion'] == 1 and event['requestId'] for event in events)
                    assert events[0]['result']['event'] == 'snapshot'
                    assert_unavailable(events[0]['result']['availability'])
                    assert events[1]['result']['event'] == 'lifecycle' and events[1]['result']['lifecycle'] == 'stopping'
                    assert events[1]['result']['sequence'] == 2 and events[1]['result']['backendAvailable'] is False
                    assert not endpoint.exists(), 'Graceful shutdown must remove its socket'
                    stopped = wait_for(lambda: (value if (value := shell_status(driver))['runtime']['phase'] in ['stopped', 'disconnected'] else None),
                                       'attached UI observes shutdown')
                    assert stopped['connected'] is False
                    wait_for(lambda: driver.script("return /stopping|stopped|disconnected|not connected/i.test(document.querySelector('#runtime-status').textContent)"), 'stopped/disconnected UI label')
                    assert driver.script("return [...document.querySelectorAll('.desktop-toolbar button')].some(e => e.disabled && e.textContent.includes('Audio mute unavailable'))")
                    report['steps'].append({'action': 'explicit-cli-shutdown-and-reap', 'reply': shutdown,
                        'runtime_pid_reaped': runtime_child.pid, 'exit_code': runtime_child.returncode,
                        'cli_events': events, 'shell_status': stopped})
                    native_button(driver, '#runtime-connect')
                    failed = wait_for(lambda: (value if (value := shell_status(driver))['runtime']['phase'] == 'disconnected' and value['runtime']['error'] else None),
                                      'connecting stopped runtime fails without autospawn')
                    assert failed['connected'] is False and failed['runtime']['event'] is None
                    assert runtime_child.poll() == 0 and not endpoint.exists()
                    report['steps'].append({'action': 'connect-stopped-no-autospawn', 'shell_status': failed})
                try:
                    driver.click('#close-window' if action == 'close-window' else '#quit-ui')
                except (urllib.error.URLError, http.client.HTTPException, RuntimeError) as error:
                    # An exiting webview can drop the click response. PID exit
                    # below is mandatory; a transport error alone never passes.
                    report['steps'].append({'action': action, 'exit_response': str(error)})
            wait_for(lambda: exited(pid), action + ' exits UI process')
            report['steps'].append({'action': action, 'status': 'passed', 'pid_exited': pid})
            pid = None
            driver.delete()
            if action != 'runtime-shutdown':
                report['steps'].append({'action': 'same-runtime-survives-ui-exit', 'case': action,
                    **runtime_alive(runtime_child, endpoint, cli, instance)})
        assert all(item['status'] == 'passed' for item in report['timing'].values()), 'Native p95 exceeds 100 ms'
        survived = [step for step in report['steps'] if step['action'] == 'same-runtime-survives-ui-exit']
        assert len(survived) == 3 and all(step['pid'] == runtime_child.pid for step in survived)
        assert runtime_child.returncode == 0
        report['runtime-survival'] = {'status': 'passed', 'pid': runtime_child.pid,
            'ui_exit_cases': [step['case'] for step in survived],
            'explicit_cli_shutdown': True, 'reaped_exit_code': runtime_child.returncode,
            'audio_backend': 'unavailable', 'audio_proof': 'unverified'}
        report['status'] = 'passed'
    except Exception as error:
        report['failure'] = repr(error)
        report['failure_traceback'] = traceback.format_exc()
        diagnostics = {'native_window': driver.window_id, 'ui_pid': pid}
        report['failure_evidence'] = diagnostics
        for label, capture in [
            ('active_native_window', lambda: xdotool('getactivewindow')),
            ('native_keyboard_focus', lambda: xdotool('getwindowfocus')),
            ('dom', lambda: input_state(driver)),
            ('shell_status', lambda: shell_status(driver)),
        ]:
            try:
                diagnostics[label] = capture()
            except Exception as diagnostic_error:
                diagnostics[label + '_error'] = repr(diagnostic_error)
        try:
            (output / 'failure-webview.png').write_bytes(base64.b64decode(driver.call('GET', '/screenshot')))
            diagnostics['screenshot'] = 'failure-webview.png'
        except Exception as diagnostic_error:
            diagnostics['screenshot_error'] = repr(diagnostic_error)
        print(f'Failure evidence: {diagnostics!r}', flush=True)
        raise
    finally:
        report['native_input_attempts'] = driver.input_attempts
        # Keep failure evidence even if a terminated driver rejects cleanup.
        (output / 'linux-webview.json').write_text(json.dumps(report, indent=2) + '\n')
        driver.delete()
        if pid and not exited(pid):
            os.kill(pid, signal.SIGTERM)
        for process in reversed(processes):
            stop(process)
        # Remove only the empty namespace created by this harness's child.
        if directory_identity and endpoint.parent.exists():
            current = endpoint.parent.stat()
            if (current.st_dev, current.st_ino) == (directory_identity.st_dev, directory_identity.st_ino):
                try:
                    endpoint.parent.rmdir()
                except OSError as error:
                    print(f'Owned runtime directory retained: {error}', flush=True)


if __name__ == '__main__':
    main(*(Path(arg).resolve() for arg in sys.argv[1:6]))
