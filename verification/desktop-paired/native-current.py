"""Run the Cargo-attested current executable, independently of historical RED."""
import json
import os
from pathlib import Path
import subprocess
from native_test_build import compile_tests

if os.environ.get('GITHUB_ACTIONS') != 'true':
    raise RuntimeError('Hosted owned native qualification only')
executable, evidence = compile_tests('apps/desktop/native-control/Cargo.toml')
evidence['sourceHead'] = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
Path('evidence').mkdir(exist_ok=True)
Path('evidence/native-current.json').write_text(json.dumps(evidence, indent=2) + '\n')
print('Current native executable attestation: ' + json.dumps(evidence), flush=True)
# Execute exactly the artifact Cargo reported for THIS source, not a binary
# selected by a shared target glob or inferred from a historical test result.
raise SystemExit(subprocess.call([str(executable)]))
