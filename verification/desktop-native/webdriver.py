"""Actual Linux release webview, trusted keys and no-tray close/relaunch checks."""
import base64
import http.client
import json
import math
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request

from evidence import inventory

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
    session = None

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
            except (urllib.error.URLError, http.client.HTTPException, RuntimeError) as error:
                print(f'Session cleanup after UI exit: {error}', flush=True)
            self.session = None


def window():
    result = subprocess.run(['xdotool', 'search', '--onlyvisible', '--name', '^Mixyard$'],
                            capture_output=True, text=True, timeout=10)
    return result.stdout.strip().splitlines()[0] if result.returncode == 0 else None


def exited(pid):
    path = Path(f'/proc/{pid}/stat')
    return not path.exists() or path.read_text().split(') ')[1].startswith('Z')


def stop(process):
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)


def main(binary, output):
    if os.environ.get('MIXYARD_NATIVE_ISOLATED') != '1':
        raise SystemExit('Use run-linux.sh: only an owned Xvfb/session bus is allowed.')
    report = inventory(binary)
    report.update(status='failed', display='owned Xvfb 1280x960, Openbox, software rendering, no tray host',
                  steps=[], timing={})
    driver = Driver()
    processes = []
    pid = None
    try:
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

        for action in ['native-close', 'close-window', 'quit-ui']:
            session = driver.request('POST', '/session', {'capabilities': {'alwaysMatch': {
                'browserName': 'wry', 'tauri:options': {'application': str(binary)}}}})
            driver.session = session['sessionId']
            report['steps'].append({'action': 'session-created', 'case': action,
                                    'url': driver.call('GET', '/url')})
            wait_for(lambda: driver.script(
                "const banner = document.querySelector('.connection-banner'); return banner && banner.textContent.includes('Desktop shell available on linux')"),
                'real shell IPC status')
            wid = wait_for(window, 'visible Mixyard native window')
            pid = int(xdotool('getwindowpid', wid))
            xdotool('windowactivate', '--sync', wid)
            report['steps'].append({'action': 'launch/relaunch', 'case': action, 'pid': pid,
                                    'url': driver.call('GET', '/url'),
                                    'controls_locked': driver.script(
                                        "return [...document.querySelectorAll('input[type=range]')].every(e => e.disabled)")})
            assert report['steps'][-1]['controls_locked'], 'Disconnected controls must stay locked'
            assert driver.script("return '__TAURI_INTERNALS__' in window"), 'Must execute the Tauri webview'
            status = driver.call('POST', '/execute/async', {'script':
                "const done = arguments[arguments.length - 1]; window.__TAURI_INTERNALS__.invoke('shell_status').then(done);",
                'args': []})
            assert status['connected'] is False and status['platform'] == 'linux'
            report['steps'][-1]['shell_status'] = status

            if action == 'native-close':
                driver.script("document.querySelector('.header-actions .quiet-button').focus();")
                xdotool('key', 'Tab')
                assert driver.script("return document.activeElement.textContent") == 'Show demo'
                xdotool('key', 'Return')
                wait_for(lambda: driver.script("return document.querySelector('.demo-tag') !== null"), 'labelled demo')
                driver.script("""
                    const range = document.querySelector('.master-gain input');
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
                        assert all(s['output'] == f"{20 * math.log10(float(s['after']) / 100):.1f} dB"
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
                (output / 'demo-webview.png').write_bytes(base64.b64decode(driver.call('GET', '/screenshot')))
                xdotool('windowsize', wid, 640, 850)
                wait_for(lambda: driver.script('return innerWidth') <= 640, 'compact native window')
                assert driver.script('return document.documentElement.scrollWidth <= innerWidth'), 'Compact overflow'
                (output / 'compact-webview.png').write_bytes(base64.b64decode(driver.call('GET', '/screenshot')))
                xdotool('key', 'Alt+F4')
            else:
                try:
                    driver.click('.app-footer button:nth-child(1)' if action == 'close-window'
                                 else '.app-footer button:nth-child(2)')
                except (urllib.error.URLError, http.client.HTTPException, RuntimeError) as error:
                    # An exiting webview can drop the click response. PID exit
                    # below is mandatory; a transport error alone never passes.
                    report['steps'].append({'action': action, 'exit_response': str(error)})
            wait_for(lambda: exited(pid), action + ' exits UI process')
            report['steps'].append({'action': action, 'status': 'passed', 'pid_exited': pid})
            pid = None
            driver.delete()
        assert all(item['status'] == 'passed' for item in report['timing'].values()), 'Native p95 exceeds 100 ms'
        report['status'] = 'passed'
    except Exception as error:
        report['failure'] = repr(error)
        raise
    finally:
        # Keep failure evidence even if a terminated driver rejects cleanup.
        (output / 'linux-webview.json').write_text(json.dumps(report, indent=2) + '\n')
        driver.delete()
        if pid and not exited(pid):
            os.kill(pid, signal.SIGTERM)
        for process in reversed(processes):
            stop(process)


if __name__ == '__main__':
    main(Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve())
