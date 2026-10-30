"""Exact historical production sources with unchanged current causal assertions."""
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import sys
from native_test_build import compile_tests

CASES = {
    'pre-vault': ('f6b1b0df5c31aef975875098b8a9a385c61aa899',
        'authority_tests::pre_vault_tests::pre_vault_old_open_cannot_supersede_a_successful_replacement',
        'CAUSAL_RED pre-vault-old-open-erased-validated-replacement'),
    'wrong-credential': ('e7eab403627600529536be7f4610b53c1610b922',
        'intent_tests::intent_wrong_credential_open_preserves_established_session',
        'CAUSAL_RED wrong-credential-open-erased-established-session'),
}
case = sys.argv[1] if len(sys.argv) == 2 else 'pre-vault'
if case not in CASES or len(sys.argv) > 2: raise RuntimeError('Unknown historical native case')
BEFORE, name, cause = CASES[case]
if os.environ.get('GITHUB_ACTIONS')!='true': raise RuntimeError('Hosted owned source qualification only')
current=Path.cwd()/'apps/desktop/native-control'
with tempfile.TemporaryDirectory(prefix='native-pre-admission-before-',dir=os.environ['RUNNER_TEMP']) as directory:
    destination=Path(directory);(destination/'src').mkdir()
    for filename in ['Cargo.toml','Cargo.lock','src/tls.rs','src/lib.rs']:
        text=subprocess.check_output(['git','show',BEFORE+':apps/desktop/native-control/'+filename],text=True)
        (destination/filename).write_text(text)
    # No production-source overlay. Only the same vault-boundary fixture and
    # unchanged pre-vault assertion used in current tests are supplied.
    files=['authority_tests.rs','pre_vault_tests.rs'] + (['intent_tests.rs'] if case=='wrong-credential' else [])
    for filename in files:
        (destination/'src'/filename).write_text((current/'src'/filename).read_text())
    executable,evidence=compile_tests(destination/'Cargo.toml',destination/'target')
    evidence['historicalSourceHead']=BEFORE
    print('Historical pre-admission isolated executable attestation: '+json.dumps(evidence),flush=True)
    result=subprocess.run([str(executable),name,'--exact','--nocapture'],capture_output=True,text=True,timeout=120)
    output=result.stdout+'\n'+result.stderr
    if result.returncode!=101 or cause not in output or not re.search(r'test '+re.escape(name)+r' \.\.\. FAILED',output) or '0 passed; 1 failed;' not in output:
        print('Historical pre-admission RED mismatch: '+json.dumps({'exit':result.returncode,'causalMarker':cause in output}));raise SystemExit(1)
    print('PASS exact historical production native case is causally RED: '+case)
