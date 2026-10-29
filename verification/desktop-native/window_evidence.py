"""Read-only evidence from the wrapper-owned Xvfb, never the operator desktop."""
import json
import os
from pathlib import Path
import re
import subprocess
import time


def command(args, timeout=2):
    try:
        result = subprocess.run(args, capture_output=True, text=True, timeout=timeout)
        return {'command': args, 'exit_code': result.returncode,
                'stdout': result.stdout[:32768], 'stderr': result.stderr[:4096]}
    except (OSError, subprocess.TimeoutExpired) as error:
        return {'command': args, 'error': str(error)}


def owned_processes(roots):
    result = command(['ps', '-eo', 'pid=,ppid=,uid=,stat=,comm='])
    if result.get('exit_code') != 0:
        return result
    rows = [line.split(None, 4) for line in result['stdout'].splitlines()]
    rows = [row for row in rows if len(row) == 5]
    selected = set(roots)
    while True:
        descendants = {int(row[0]) for row in rows if int(row[1]) in selected}
        expanded = selected | descendants
        if expanded == selected:
            break
        selected = expanded
    return [{'pid': int(pid), 'ppid': int(ppid), 'uid': int(uid), 'state': state, 'name': name}
            for pid, ppid, uid, state, name in rows if int(pid) in selected]


def capture(output, label, processes):
    if os.environ.get('MIXYARD_NATIVE_ISOLATED') != '1':
        raise RuntimeError('Native evidence requires the wrapper-owned isolated display')
    if not re.fullmatch(r'[a-z0-9-]+', label):
        raise ValueError('Invalid evidence label')
    output = Path(output)
    report = {
        'monotonic_seconds': time.monotonic(), 'display': os.environ.get('DISPLAY'),
        'harness_pid': os.getpid(), 'uid': os.geteuid(),
        'started_processes': [{'pid': process.pid, 'exit_code': process.poll()} for process in processes],
        'processes': owned_processes([os.getpid(), *(process.pid for process in processes)]),
        'root': command(['xprop', '-root', '_NET_SUPPORTING_WM_CHECK', '_NET_CLIENT_LIST', '_NET_ACTIVE_WINDOW']),
        'tree': command(['xwininfo', '-root', '-tree']),
        'title_all': command(['xdotool', 'search', '--name', '^Mixyard$']),
        'title_visible': command(['xdotool', 'search', '--onlyvisible', '--name', '^Mixyard$']),
        'active': command(['xdotool', 'getactivewindow']),
        'focus': command(['xdotool', 'getwindowfocus']),
    }
    # Include unnamed/unmanaged windows too; otherwise the title selector can
    # erase the very evidence needed when no matching visible window is found.
    ids = list(dict.fromkeys(re.findall(r'0x[0-9a-fA-F]+', report['tree'].get('stdout', ''))))
    report['window_count'] = len(ids)
    report['windows'] = []
    deadline = time.monotonic() + 8
    for window in ids[:32]:
        if time.monotonic() >= deadline:
            break
        report['windows'].append({
            'id': window,
            'properties': command(['xprop', '-id', window, '_NET_WM_PID', '_NET_WM_NAME', 'WM_NAME',
                                   'WM_CLASS', 'WM_STATE', '_NET_WM_STATE', '_NET_WM_DESKTOP'], timeout=1),
            'geometry': command(['xwininfo', '-id', window], timeout=1),
        })
    report['windows_truncated'] = len(report['windows']) != len(ids)
    # Capture the owned X root independently of WebDriver: a stalled WebKit
    # screenshot must not hide an actual mapping/focus/window-manager failure.
    screenshot = output / f'{label}-root.png'
    report['root_screenshot'] = command(['import', '-window', 'root', str(screenshot)], timeout=3)
    output.joinpath(f'{label}-native.json').write_text(json.dumps(report, indent=2) + '\n')
    return report
