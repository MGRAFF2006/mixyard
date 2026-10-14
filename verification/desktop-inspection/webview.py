"""Focused inspection run in the real release webview; no audio/runtime needed."""
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import traceback
import urllib.error

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'verification/desktop-native'))
from webdriver import Driver, stop, wait_for, window, xdotool  # noqa: E402
from evidence import disk_guard, inventory  # noqa: E402


def main(binary, output):
    assert os.environ.get('MIXYARD_INSPECTION_ISOLATED') == '1', 'Use run-linux.sh in an owned container'
    disk_guard()
    report = inventory(binary)
    report.update(status='failed', steps=[], boundary=(
        'Actual release Linux Tauri/WebKit content import, core IPC and rendered diagnostics. '
        'Simulated snapshots, injected text/file content and trusted Inspect keyboard activation; '
        'not an OS file-picker, physical audio, live discovery, screen reader or other-platform GUI proof.'))
    report['inspection_source_files'] = {
        str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
        for p in sorted((ROOT / 'verification/desktop-inspection').glob('*')) if p.is_file()
    }
    driver, processes = Driver(), []
    try:
        for command, name in [(['openbox'], 'wm'), (['tauri-driver'], 'driver')]:
            with (output / f'{name}.log').open('w') as log:
                processes.append(subprocess.Popen(command, stdout=log, stderr=log, start_new_session=True))
        def ready():
            try:
                driver.request('GET', '/status')
                return True
            except urllib.error.URLError:
                return False
        wait_for(ready, 'tauri-driver')
        session = driver.request('POST', '/session', {'capabilities': {'alwaysMatch': {
            'browserName': 'wry', 'tauri:options': {'application': str(binary)}}}})
        driver.session = session['sessionId']
        wait_for(lambda: driver.script("return !!document.querySelector('#snapshot-inspection')"), 'inspection panel')
        wid = wait_for(window, 'owned window')
        xdotool('windowactivate', '--sync', wid)
        driver.click('#snapshot-inspection > summary')
        assert driver.script("return '__TAURI_INTERNALS__' in window")
        assert driver.call('GET', '/url').startswith('tauri://localhost')
        driver.script("""
          window.inspectionCalls = 0;
          const callbacks = window.__TAURI_INTERNALS__.callbacks;
          const realSet = callbacks.set.bind(callbacks);
          callbacks.set = (id, callback) => realSet(id, data => {
            if (typeof data !== 'string' || !data.includes('"provenance":"user-import"')) return callback(data);
            window.inspectionCalls++;
            window.lastRealInspection = data;
            if (window.holdInspection) { window.releaseInspection = () => callback(data); return; }
            return callback(data);
          });
        """)
        config = json.loads((ROOT / 'examples/config.json').read_text())
        inv = json.loads((ROOT / 'examples/inventory.json').read_text())

        def paste(c, i):
            driver.script(f"""
              for (const [id, text] of {json.dumps([['inspection-config', json.dumps(c)], ['inspection-inventory', json.dumps(i)]])}) {{
                const el = document.getElementById(id); el.value = text;
                el.dispatchEvent(new Event('input', {{bubbles:true}}));
              }}
            """)
            wait_for(lambda: driver.script("return !document.querySelector('#inspection-result')"), 'previous result cleared')

        def inspect(accepted):
            driver.script("document.querySelector('#inspect-snapshot').scrollIntoView({block:'center'}); document.querySelector('#inspect-snapshot').focus();")
            xdotool('key', 'Return')
            wait_for(lambda: driver.script("return !!document.querySelector('#inspection-result')"), 'rendered report')
            raw = json.loads(driver.script('return window.lastRealInspection'))
            assert raw['report']['planAvailable'] is accepted
            assert driver.script("return document.querySelector('#runtime-status').textContent.includes('Not connected')")
            assert driver.script("return [...document.querySelectorAll('input[type=range]')].every(e=>e.disabled)")
            return raw

        paste(config, inv)
        accepted = inspect(True)
        (output / 'accepted-core-report.json').write_text(json.dumps(accepted, indent=2) + '\n')
        text = driver.script("return document.querySelector('#inspection-result').textContent")
        for expected in ['freshness unverified', 'simulated', 'send-zero', 'Configured potential sharing', 'Applied sharing: unknown', 'excluded-call-return', 'capacity-unknown', 'Conservative headroom']:
            assert expected in text, expected
        assert driver.script("return document.querySelectorAll('#inspection-result tbody tr').length") == len(config['sources']) * len(config['outputs'])
        report['steps'].append({'case': 'accepted-import', 'all_paths': 28, 'trusted_activation': 'xdotool Return', 'native_reply': 'accepted-core-report.json'})

        rejected = copy.deepcopy(config)
        rejected['outputs'][2]['allowMicMonitor'] = False
        paste(rejected, inv)
        raw = inspect(False)
        assert all(p['plannedEffectiveCoefficient'] is None for o in raw['report']['outputs'] for p in o['paths'])
        assert 'mic-monitor-consent-required' in driver.script("return document.querySelector('#inspection-result').textContent")
        (output / 'rejected-core-report.json').write_text(json.dumps(raw, indent=2) + '\n')
        report['steps'].append({'case': 'rejected-monitor-consent-including-zero', 'coefficients': 'all null'})

        headroom = copy.deepcopy(config)
        headroom['sends'][1]['gain'] = 1
        paste(headroom, inv)
        inspect(False)
        assert 'headroom' in driver.script("return document.querySelector('#inspection-preflight').parentElement.textContent")
        report['steps'].append({'case': 'headroom-rejection-visible'})

        uncertain = copy.deepcopy(inv)
        uncertain['sources'] = [s for s in uncertain['sources'] if s['selector']['kind'] != 'mic']
        uncertain['sources'][0]['format'] = None
        ambiguous = copy.deepcopy(uncertain['sources'][1]); ambiguous['observedId'] += ':second'
        uncertain['sources'].append(ambiguous)
        paste(config, uncertain)
        inspect(True)
        text = driver.script("return document.querySelector('#inspection-result').textContent")
        for expected in ['source-missing', 'source-ambiguous', 'format-unknown']:
            assert expected in text, expected
        report['steps'].append({'case': 'missing-ambiguous-unknown-format-reasons'})

        paste(config, inv)
        driver.script('window.holdInspection = true; window.releaseInspection = null;')
        driver.click('#inspect-snapshot')
        wait_for(lambda: driver.script('return !!window.releaseInspection'), 'held real core reply')
        calls = driver.script('return window.inspectionCalls')
        assert driver.script("return document.querySelector('#inspect-snapshot').disabled")
        paste(rejected, inv)
        driver.script('window.releaseInspection(); window.holdInspection = false;')
        wait_for(lambda: driver.script("return !document.querySelector('#inspect-snapshot').disabled"), 'completed stale reply')
        assert driver.script("return !document.querySelector('#inspection-result') && !!document.querySelector('#inspection-empty')")
        assert driver.script('return window.inspectionCalls') == calls
        report['steps'].append({'case': 'input-change-suppresses-delayed-real-native-response', 'overlap': 'button disabled'})

        paste(config, inv)
        inspect(True)
        driver.script("""
          const file = new File(['x'.repeat(131073)], 'oversized-fixture.json', {type:'application/json'});
          const transfer = new DataTransfer(); transfer.items.add(file);
          const input = document.getElementById('inspection-config-file'); input.files = transfer.files;
          input.dispatchEvent(new Event('change', {bubbles:true}));
        """)
        wait_for(lambda: driver.script("return !!document.querySelector('#inspection-error')"), 'oversized file rejection')
        assert driver.script("return !document.querySelector('#inspection-result') && document.querySelector('#inspection-config').value === ''")
        report['steps'].append({'case': 'oversized-selected-content-clears-result-and-input', 'file_origin': 'explicit synthetic File/DataTransfer, not OS picker'})

        paste(config, inv)
        driver.script("const e=document.querySelector('#inspection-config');e.value='{';e.dispatchEvent(new Event('input',{bubbles:true}));")
        calls = driver.script('return window.inspectionCalls')
        driver.click('#inspect-snapshot')
        wait_for(lambda: driver.script("return !!document.querySelector('#inspection-error')"), 'malformed input')
        assert driver.script('return window.inspectionCalls') == calls
        assert driver.script("return !document.querySelector('#inspection-result')")
        report['steps'].append({'case': 'malformed-paste-no-invoke-no-previous-result'})

        driver.call('POST', '/window/rect', {'width': 640, 'height': 850})
        assert driver.script('return document.documentElement.scrollWidth <= innerWidth'), 'compact overflow'
        report['steps'].append({'case': '640px-compact-no-page-overflow'})
        driver.click('#clear-snapshots')
        assert driver.script("return !document.querySelector('#inspection-result') && document.querySelector('#inspection-config').value === '' && document.querySelector('#inspection-inventory').value === ''")
        report['status'] = 'passed'
    except BaseException:
        report['failure'] = traceback.format_exc()
        raise
    finally:
        driver.delete()
        for process in reversed(processes):
            stop(process)
        report['free_bytes_after'] = shutil.disk_usage('.').free
        (output / 'linux-inspection.json').write_text(json.dumps(report, indent=2) + '\n')
        disk_guard()


if __name__ == '__main__':
    directory = Path(sys.argv[2]); directory.mkdir(parents=True, exist_ok=True)
    main(Path(sys.argv[1]).resolve(), directory.resolve())
