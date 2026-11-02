"""Select all automatic Linux lanes. Uncertain history or shared paths run all.

The job always exists, so a path miss cannot leave its required gate pending.
This reads Git objects, not the GitHub changed-files API (which can truncate).
"""
import fnmatch
import json
import os
from pathlib import Path
import re
import subprocess

CONFIG = Path(__file__).with_name('linux-fixtures-paths.json')
SHA = re.compile(r'[0-9a-f]{40}')


def git(repo, *args):
    return subprocess.check_output(['git', '-C', str(repo), *args], stderr=subprocess.DEVNULL)


def changed_paths(repo, event_name, event):
    """None means uncertain: never turn an incomplete comparison into a skip."""
    try:
        if event_name == 'pull_request':
            base = event['pull_request']['base']['sha']
            head = event['pull_request']['head']['sha']
        elif event_name == 'push':
            base, head = event['before'], event['after']
        else:
            return None
        if any(not isinstance(ref, str) or not SHA.fullmatch(ref) or ref == '0' * 40
               for ref in (base, head)):
            return None
        if event_name == 'pull_request':
            base = git(repo, 'merge-base', base, head).decode().strip()
        # Disable rename detection: both the old and new paths must participate.
        raw = git(repo, 'diff', '--no-renames', '--name-only', '-z', base, head, '--')
        return [p.decode('utf-8') for p in raw.split(b'\0') if p]
    except (KeyError, TypeError, ValueError, UnicodeError, subprocess.CalledProcessError):
        return None


def select(paths, config):
    if paths is None or not paths:
        return set(config)
    selected = set()
    for path in paths:
        # Unmapped files include CI/actions, manifests, shared code and config.
        # fnmatch's broader '*' matching is deliberately conservative here.
        matches = {lane for lane, patterns in config.items()
                   if any(fnmatch.fnmatchcase(path, pattern) for pattern in patterns)}
        if (path.startswith(('.github/', 'src/', 'tests/', 'examples/', 'scripts/'))
                or '/' not in path
                or Path(path).suffix in ('.toml', '.lock', '.yaml', '.yml')
                or Path(path).name in ('package.json', 'tsconfig.json')
                or not matches):
            return set(config)
        selected.update(matches)
    return selected


def main():
    config = json.loads(CONFIG.read_text())
    try:
        event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())
    except (KeyError, OSError, ValueError):
        event = {}
    paths = changed_paths(Path.cwd(), os.environ.get('GITHUB_EVENT_NAME'), event)
    selected = select(paths, config)
    print('Selected Linux fixtures: ' + ', '.join(sorted(selected)))
    with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
        for lane in config:
            output.write(f"{lane.replace('-', '_')}={'true' if lane in selected else 'false'}\n")


if __name__ == '__main__':
    main()
