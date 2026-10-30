"""Exact f6 production source + current vault-boundary barrier test: causal RED."""
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
from native_test_build import compile_tests

BEFORE='f6b1b0df5c31aef975875098b8a9a385c61aa899'
if os.environ.get('GITHUB_ACTIONS')!='true': raise RuntimeError('Hosted owned source qualification only')
current=Path.cwd()/'apps/desktop/native-control'
with tempfile.TemporaryDirectory(prefix='native-pre-admission-before-',dir=os.environ['RUNNER_TEMP']) as directory:
    destination=Path(directory);(destination/'src').mkdir()
    for name in ['Cargo.toml','Cargo.lock','src/tls.rs','src/lib.rs']:
        text=subprocess.check_output(['git','show',BEFORE+':apps/desktop/native-control/'+name],text=True)
        (destination/name).write_text(text)
    # No production-source overlay. Only the same vault-boundary fixture and
    # unchanged pre-vault assertion used in current tests are supplied.
    for name in ['authority_tests.rs','pre_vault_tests.rs']:
        (destination/'src'/name).write_text((current/'src'/name).read_text())
    executable,evidence=compile_tests(destination/'Cargo.toml',destination/'target')
    evidence['historicalSourceHead']=BEFORE
    print('Historical pre-admission isolated executable attestation: '+json.dumps(evidence),flush=True)
    name='authority_tests::pre_vault_tests::pre_vault_old_open_cannot_supersede_a_successful_replacement'
    result=subprocess.run([str(executable),name,'--exact','--nocapture'],capture_output=True,text=True,timeout=120)
    output=result.stdout+'\n'+result.stderr
    cause='CAUSAL_RED pre-vault-old-open-erased-validated-replacement'
    if result.returncode!=101 or cause not in output or not re.search(r'test '+re.escape(name)+r' \.\.\. FAILED',output) or '0 passed; 1 failed;' not in output:
        print('Historical pre-admission RED mismatch: '+json.dumps({'exit':result.returncode,'causalMarker':cause in output}));raise SystemExit(1)
    print('PASS exact-f6 production pre-vault scheduling regression is causally RED')
