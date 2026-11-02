"""A tolerated lane failure must still fail the single required gate."""
import json
import os


def failures(selected, outcomes):
    return [lane for lane, enabled in selected.items()
            if enabled == 'true' and outcomes.get(lane.upper()) != 'success']


if __name__ == '__main__':
    selected = json.loads(os.environ['SELECTED'])
    failed = failures(selected, os.environ)
    if not selected or failed:
        raise SystemExit('Missing selection or unsuccessful fixture lanes: ' + ', '.join(failed))
