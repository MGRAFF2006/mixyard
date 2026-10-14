"""Actual Linux release shell reading a temporary CLI-created default store."""
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
from evidence import disk_guard, inventory, binary_info  # noqa: E402


def main(binary, cli, output):
    assert os.environ.get('MIXYARD_PROFILES_ISOLATED') == '1', 'Use run-linux.sh inside an owned container'
    disk_guard()
    report = inventory(binary)
    report.update(status='failed', cli_binary=binary_info(cli), steps=[], boundary=(
        'Actual release Linux Tauri/WebKit default-store reads and profiles::preview; '
        'owned temporary CLI-created local profiles and simulated imported inventory. '
        'Injected inventory DOM content; trusted Return activation. No OS file picker, '
        'physical audio/live setup, tray, screen reader or Windows/macOS GUI execution.'))
    report['profile_verification_sources'] = {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
        for p in sorted((ROOT / 'verification/desktop-profiles').glob('*')) if p.is_file()}
    driver, processes = Driver(), []
    store = Path(os.environ['XDG_DATA_HOME']) / 'mixyard/profiles'
    config = json.loads((ROOT / 'examples/config.json').read_text())
    inv = json.loads((ROOT / 'examples/inventory.json').read_text())
    fixtures = Path(os.environ['XDG_RUNTIME_DIR'])

    def command(*args):
        r = subprocess.run([str(cli), 'profiles', *map(str, args)], capture_output=True, text=True, timeout=10)
        assert r.returncode == 0, r.stderr
        return json.loads(r.stdout)

    def fixture(name, value):
        p = fixtures / name; p.write_text(json.dumps(value)); return p

    def state():
        return {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in store.iterdir()}

    try:
        for args, name in [(['openbox'], 'wm'), (['tauri-driver'], 'driver')]:
            with (output / f'{name}.log').open('w') as log:
                processes.append(subprocess.Popen(args, stdout=log, stderr=log, start_new_session=True))
        def ready():
            try:
                driver.request('GET', '/status'); return True
            except urllib.error.URLError:
                return False
        wait_for(ready, 'tauri-driver')
        session = driver.request('POST', '/session', {'capabilities': {'alwaysMatch': {
            'browserName': 'wry', 'tauri:options': {'application': str(binary)}}}})
        driver.session = session['sessionId']
        wait_for(lambda: driver.script("return !!document.querySelector('#local-profiles')"), 'profile panel')
        xdotool('windowactivate', '--sync', wait_for(window, 'owned window'))
        assert driver.call('GET', '/url').startswith('tauri://localhost')
        driver.click('#local-profiles > summary')
        driver.script("""
          window.profileCalls = 0;
          const callbacks = window.__TAURI_INTERNALS__.callbacks;
          const realSet = callbacks.set.bind(callbacks);
          callbacks.set = (id, callback) => realSet(id, data => {
            if (typeof data !== 'string' || !data.includes('"mode":"local-profile-preview"')) return callback(data);
            window.profileCalls++; window.lastProfileReply = data;
            if (window.holdProfile) { window.releaseProfile = () => callback(data); return; }
            return callback(data);
          });
        """)

        def refresh():
            driver.click('#profiles-refresh')
            wait_for(lambda: driver.script("return !!document.querySelector('#profiles-store-state')"), 'store read')
            assert driver.script("return document.querySelector('#profile-selection').value === '' && !document.querySelector('#profile-result')")
            return json.loads(driver.script('return window.lastProfileReply'))

        def paste(inventory_value):
            text = json.dumps(json.dumps(inventory_value))
            driver.script(f"const e=document.querySelector('#profile-inventory');e.value={text};e.dispatchEvent(new Event('input',{{bubbles:true}}));")
            assert driver.script("return !document.querySelector('#profile-result') && document.querySelector('#profile-selection').value === ''")

        def select(identity):
            driver.script(f"const e=document.querySelector('#profile-selection');e.value={json.dumps(identity)};e.dispatchEvent(new Event('change',{{bubbles:true}}));")
            assert driver.script("return !document.querySelector('#profile-result')")

        def preview():
            driver.script("document.querySelector('#profile-preview').scrollIntoView({block:'center'});document.querySelector('#profile-preview').focus();")
            xdotool('key', 'Return')
            wait_for(lambda: driver.script("return !!document.querySelector('#profile-result')"), 'real core preview rendered')
            assert driver.script("return document.querySelector('#profile-apply').disabled && document.querySelector('#runtime-status').textContent.includes('Not connected')")
            return json.loads(driver.script('return window.lastProfileReply'))

        missing = refresh()
        assert missing['storeState'] == 'missing' and not store.exists()
        report['steps'].append({'case': 'missing-default-store-no-creation', 'reply': missing})
        command('draft', 0, fixture('before.json', config))
        empty = refresh(); assert empty['storeState'] == 'present' and empty['profiles'] == []
        report['steps'].append({'case': 'empty-present-store-distinct-from-missing'})
        candidate = copy.deepcopy(config)
        candidate['sources'][0]['selector']['deviceUid'] = 'fixture:missing-profile-mic'
        candidate['sends'][0]['gain'] = 0.25
        candidate['mixes'][0]['name'] = 'Saved mix ♥'; candidate['mixes'][0]['master']['gain'] = 0.5
        candidate['outputs'] = [o for o in candidate['outputs'] if o['id'] != 'headphones']
        candidate['mediaControls'] = []
        name = '../夜 / same name ♥'
        saved = command('save', 1, name, fixture('candidate.json', candidate))
        identity = saved['result']['profiles'][0]['id']
        command('save', 2, name, fixtures / 'before.json')
        before = state(); listed = refresh()
        assert len(listed['profiles']) == 2 and listed['profiles'][0]['name'] == listed['profiles'][1]['name']
        assert listed['profiles'][0]['id'] != listed['profiles'][1]['id']
        paste(inv); select(identity)
        assert driver.script("return document.querySelector('#profile-show').textContent.includes('fixture:missing-profile-mic')")
        native = preview()
        authoritative = command('preview-profile', identity, fixture('inventory.json', inv))
        assert native['preview'] == authoritative['result'] and native['storeRevision'] == authoritative['storeRevision'] == 3
        assert all(native['preview']['changes'][k] for k in ['sources', 'sends', 'mixes', 'masters', 'outputs', 'mediaControls'])
        assert any(p['output'] == 'headphones' for p in native['preview']['affectedSensitivePathsBefore'])
        assert native['preview']['unavailableIdentities']['sources']
        assert state() == before
        (output / 'accepted-native-preview.json').write_text(json.dumps(native, indent=2) + '\n')
        (output / 'authoritative-cli-preview.json').write_text(json.dumps(authoritative, indent=2) + '\n')
        report['steps'].append({'case': 'CLI-created-duplicate-names-stable-selection-six-change-kinds-exact-core-equivalence', 'storeRevision': 3, 'profileId': identity, 'unchanged_files': before})

        command('rename', 3, identity, 'Changed in CLI')
        driver.click('#profile-preview')
        wait_for(lambda: driver.script("return !!document.querySelector('#profiles-error')"), 'changed-store conflict')
        assert 'store-conflict' in driver.script("return document.querySelector('#profiles-error').textContent")
        assert driver.script("return !document.querySelector('#profile-result') && !document.querySelector('#profiles-store-state') && document.querySelector('#profile-selection').value === ''")
        report['steps'].append({'case': 'preview-rereads-store-rejects-CLI-change-clears-selection-and-snapshot'})
        refresh(); paste(inv); select(identity)
        driver.script('window.holdProfile=true;window.releaseProfile=null;')
        driver.click('#profile-preview')
        wait_for(lambda: driver.script('return !!window.releaseProfile'), 'held actual core response')
        assert driver.script("return document.querySelector('#profile-preview').disabled && document.querySelector('#profiles-refresh').disabled")
        select(listed['profiles'][1]['id'])
        driver.script('window.releaseProfile();window.holdProfile=false;')
        wait_for(lambda: driver.script("return !document.querySelector('#profiles-refresh').disabled"), 'stale response complete')
        assert driver.script("return !document.querySelector('#profile-result')")
        report['steps'].append({'case': 'selection-change-suppresses-delayed-real-reply-single-admission-buttons'})
        paste(inv); select(identity); preview()
        paste(inv)
        assert driver.script("return !document.querySelector('#profile-result') && document.querySelector('#profile-selection').value === ''")
        report['steps'].append({'case': 'inventory-edit-clears-selection-and-preview'})
        rejected = copy.deepcopy(inv); rejected['limits']['maxOutputRoutes'] = 0
        paste(rejected); select(identity); driver.click('#profile-preview')
        wait_for(lambda: driver.script("return !!document.querySelector('#profiles-error')"), 'core policy rejection')
        assert 'invalid-policy' in driver.script("return document.querySelector('#profiles-error').textContent")
        assert driver.script("return !document.querySelector('#profile-result') && document.querySelector('#profile-selection').value === ''")
        report['steps'].append({'case': 'authoritative-policy-failure-clears-old-results'})

        disk_guard()
        data = store / 'profiles.json'; preserved = data.read_bytes()
        for marker in ['write.lock', 'pending.json']:
            p = store / marker; p.write_text('owned test marker'); p.chmod(0o600)
            driver.click('#profiles-refresh')
            wait_for(lambda: driver.script("return !!document.querySelector('#profiles-error')"), 'busy/recovery marker')
            assert 'store-busy' in driver.script("return document.querySelector('#profiles-error').textContent")
            assert p.read_text() == 'owned test marker' and data.read_bytes() == preserved
            p.unlink()
        data.write_text('{')
        driver.click('#profiles-refresh')
        wait_for(lambda: driver.script("return !!document.querySelector('#profiles-error')"), 'corrupt store')
        assert 'corrupt-store' in driver.script("return document.querySelector('#profiles-error').textContent")
        assert driver.script("return !document.querySelector('#profiles-empty') && !document.querySelector('#profile-result') && document.querySelector('#profile-selection').options.length === 1")
        assert data.read_text() == '{'
        data.write_bytes(preserved)
        report['steps'].append({'case': 'busy-pending-and-corrupt-store-no-empty-fallback-no-repair'})
        refresh(); paste(inv); select(identity); preview()
        driver.call('POST', '/window/rect', {'width': 640, 'height': 850})
        assert driver.script('return document.documentElement.scrollWidth <= innerWidth'), 'compact overflow'
        assert driver.script("return [...document.querySelectorAll('input[type=range]')].every(e=>e.disabled) && document.querySelector('#profile-apply').disabled")
        report['steps'].append({'case': '640px-layout-apply-and-route-controls-remain-locked'})
        report['status'] = 'passed'
    except BaseException:
        report['failure'] = traceback.format_exc()
        raise
    finally:
        driver.delete()
        for process in reversed(processes): stop(process)
        report['free_bytes_after'] = shutil.disk_usage('.').free
        (output / 'linux-profiles.json').write_text(json.dumps(report, indent=2) + '\n')
        disk_guard()


if __name__ == '__main__':
    main(Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve(), Path(sys.argv[3]).resolve())
