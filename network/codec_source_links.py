"""Run the production Cargo selector for a host-only direct-source proof.

Cargo directives are data, never shell code. The selector owns feature-scoped
validation, custom artifact names, link kinds and private exact-byte staging.
"""
import subprocess
import sys


def parse_directives(output):
    flags = []
    for line in output.splitlines():
        if line.startswith('cargo:rustc-link-search='):
            flags += ['-L', line.removeprefix('cargo:rustc-link-search=')]
        elif line.startswith('cargo:rustc-link-lib='):
            flags += ['-l', line.removeprefix('cargo:rustc-link-lib=')]
        elif line.startswith(('cargo:rerun-if-env-changed=', 'cargo:rerun-if-changed=')):
            continue
        else:
            raise ValueError(f'unsupported codec selector directive: {line!r}')
    return flags


def selected_links(root, target, features, env, run):
    if not features:
        return []  # Raw/default proof never compiles or invokes the FFI selector.
    output = target / 'selected-libraries'
    output.mkdir(exist_ok=True)
    selector = target / 'codec-link-setup'
    run(['rustc', '--edition=2024', str(root / 'codecs/build.rs'),
         '-Dwarnings', '-Cdebuginfo=0', '-Copt-level=s', '-Cstrip=symbols',
         '-o', str(selector)])
    selection_env = {key: value for key, value in env.items()
                     if not key.startswith('CARGO_FEATURE_SYSTEM_')}
    selection_env.update(OUT_DIR=str(output), CARGO_CFG_TARGET_OS={
        'linux': 'linux', 'darwin': 'macos', 'win32': 'windows',
    }[sys.platform], CARGO_CFG_TARGET_ENV='msvc' if sys.platform == 'win32' else '')
    for feature in features:
        selection_env['CARGO_FEATURE_' + feature.upper().replace('-', '_')] = '1'
    result = subprocess.run([str(selector)], env=selection_env, check=True,
                            capture_output=True, text=True, timeout=30)
    (target / 'codec-link-directives.txt').write_text(result.stdout)
    return parse_directives(result.stdout)
