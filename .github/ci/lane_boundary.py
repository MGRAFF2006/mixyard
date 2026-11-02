"""Restore exported action state and give each lane a declared command PATH.

Runner PATH commands cannot be undone by writing PATH to GITHUB_ENV: the runner
prepends its own list again. Command shells therefore construct PATH explicitly.
Tool installations/caches can be shared; workspace and temporary build outputs
are reset separately. This is an environment boundary, not a security sandbox.
"""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import uuid


def state_path():
    # Keep this outside the workspace cleaned by each lane's checkout.
    return Path(os.environ['GITHUB_WORKSPACE']).parent / ('.mixyard-linux-environment-' + os.environ['GITHUB_RUN_ID'] + '-' + os.environ['GITHUB_RUN_ATTEMPT'] + '.json')


def restore(baseline, current):
    return {key: baseline.get(key, '') for key in baseline.keys() | current.keys()
            if not key.startswith(('GITHUB_', 'RUNNER_', 'ACTIONS_'))
            and baseline.get(key) != current.get(key)}


def command_environment(baseline, current):
    env = dict(current)
    for key in tuple(env):
        if env[key] == '' and key not in baseline:
            del env[key]
    if env.get('LANE_ID') and not env.get('LANE_TEMP'):
        raise ValueError('Missing lane temporary directory')
    if env.get('LANE_TEMP'):
        # RUNNER_* is protected in Actions. Remap only the child command's
        # environment; setup/upload actions keep the runner's real context.
        env['RUNNER_TEMP'] = env['LANE_TEMP']
    paths = []
    if env.get('LANE_PYTHON') == 'true' and env.get('pythonLocation'):
        paths.extend([env['pythonLocation'] + '/bin', env['pythonLocation']])
    if env.get('LANE_NODE') == 'true' and env.get('LANE_NODE_VERSION'):
        version = env['LANE_NODE_VERSION'].removeprefix('v')
        if not re.fullmatch(r'\d+\.\d+\.\d+', version):
            raise ValueError('Invalid setup-node selected version')
        arch = {'X64': 'x64', 'ARM64': 'arm64'}[env['RUNNER_ARCH']]
        paths.append(f"{env['RUNNER_TOOL_CACHE']}/node/{version}/{arch}/bin")
    if env.get('LANE_PNPM') == 'true' and env.get('PNPM_HOME'):
        # pnpm/action-setup exports PNPM_HOME via GITHUB_ENV.
        paths.extend([env['PNPM_HOME'] + '/bin', env['PNPM_HOME']])
    paths.append(str(Path(baseline.get('CARGO_HOME', baseline['HOME'] + '/.cargo')) / 'bin'))
    paths.append(baseline['PATH'])
    env['PATH'] = os.pathsep.join(paths)
    return env


def main():
    path = state_path()
    mode = sys.argv[1]
    if mode == 'snapshot':
        # Ephemeral runner state, never uploaded, printed, or committed.
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, 'w') as output:
            json.dump(dict(os.environ), output)
        return
    baseline = json.loads(path.read_text())
    if mode == 'restore':
        with open(os.environ['GITHUB_ENV'], 'a') as output:
            for key, value in restore(baseline, dict(os.environ)).items():
                delimiter = 'boundary_' + uuid.uuid4().hex
                output.write(f'{key}<<{delimiter}\n{value}\n{delimiter}\n')
        return
    if mode == 'clean':
        path.unlink()
        return
    if mode != 'shell':
        raise SystemExit('Unknown boundary operation')
    env = command_environment(baseline, dict(os.environ))
    shell, script = sys.argv[2:]
    command = ['bash', '--noprofile', '--norc', '-eo', 'pipefail', script] if shell == 'bash' else ['python', script]
    result = subprocess.run(command, env=env)
    raise SystemExit(result.returncode if result.returncode >= 0 else 128 - result.returncode)


if __name__ == '__main__':
    main()
