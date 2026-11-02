"""Only explicitly inapplicable qualification consumers may be skipped."""
import json
import os


def gate_failures(selected, steps):
    failed = []
    inventory = steps.get('qualification_offline', {})
    for lane, enabled in selected.items():
        if enabled != 'true':
            continue
        key = {'qualification_core': 'core_present', 'qualification_linux': 'linux_present'}.get(lane)
        if key and inventory.get('outcome') == 'success' and inventory.get('outputs', {}).get(key) == 'false':
            continue
        if steps.get(lane, {}).get('outcome') != 'success':
            failed.append(lane)
    return failed


if __name__ == '__main__':
    selected = json.loads(os.environ['SELECTED'])
    failed = gate_failures(selected, json.loads(os.environ['OUTCOMES']))
    if not selected or failed:
        raise SystemExit('Missing selection or unsuccessful Linux lanes: ' + ', '.join(failed))
