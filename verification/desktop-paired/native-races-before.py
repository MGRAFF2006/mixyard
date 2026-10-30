"""Hosted-only causal RED check: exact b2cb production source + scheduling hooks.
No alternative state machine, fake TLS, changed assertions or production repair.
"""
import os
from pathlib import Path
import re
import subprocess
import tempfile

BEFORE='b2cb91d288e4fa25b6ced337d0268b5537bb6008'
if os.environ.get('GITHUB_ACTIONS')!='true': raise RuntimeError('Hosted owned source qualification only')
root=Path.cwd()
current=root/'apps/desktop/native-control'

def source(path):
    return subprocess.check_output(['git','show',BEFORE+':apps/desktop/native-control/'+path],text=True)

def replace(text,old,new):
    if text.count(old)!=1: raise RuntimeError('Historical scheduling anchor changed')
    return text.replace(old,new)

with tempfile.TemporaryDirectory(prefix='native-race-before-',dir=os.environ['RUNNER_TEMP']) as directory:
    destination=Path(directory); (destination/'src').mkdir()
    for name in ['Cargo.toml','Cargo.lock','src/tls.rs']:
        (destination/name).write_text(source(name))
    text=source('src/lib.rs')
    text=replace(text,'    session: Mutex<Option<Session>>,','    session: Mutex<Option<Session>>,\n    #[cfg(test)]\n    hook: Mutex<Option<authority_tests::Hook>>,')
    text=replace(text,'            session: Mutex::new(None),','            session: Mutex::new(None),\n            #[cfg(test)]\n            hook: Mutex::new(None),')
    anchor='    }\n}\n\npub struct Paired'
    callback='    }\n    #[cfg(test)]\n    fn checkpoint(&self, stage: authority_tests::Checkpoint, generation: u64) {\n        let callback = self.hook.lock().unwrap().clone();\n        if let Some(callback) = callback { callback(stage, generation); }\n    }\n}\n\npub struct Paired'
    text=replace(text,anchor,callback)
    # The real old admission gaps: pause AFTER generation increment/CAS and
    # BEFORE its separately locked clear. Current source clears atomically
    # during admission, then reaches this same scheduling checkpoint.
    anchor='        let generation = host.generation.fetch_add(1, Ordering::SeqCst) + 1;'
    text=replace(text,anchor,anchor+'\n        #[cfg(test)]\n        host.checkpoint(authority_tests::Checkpoint::OpenAdmitted, generation);')
    anchor='                .is_ok()\n            {\n                *host'
    text=replace(text,anchor,'                .is_ok()\n            {\n                #[cfg(test)]\n                host.checkpoint(authority_tests::Checkpoint::DisconnectAdmitted, current + 1);\n                *host')
    anchor='                self.fresh(&host, generation)?;\n                *host.session.lock().map_err(|_| "Session unavailable")? = Some(session);'
    text=replace(text,anchor,'                self.fresh(&host, generation)?;\n                #[cfg(test)]\n                host.checkpoint(authority_tests::Checkpoint::OpenPublication, generation);\n                *host.session.lock().map_err(|_| "Session unavailable")? = Some(session);')
    text+='\n#[cfg(test)]\nmod authority_tests;\n'
    (destination/'src/lib.rs').write_text(text)
    (destination/'src/authority_tests.rs').write_text((current/'src/authority_tests.rs').read_text())
    command=['cargo','test','--locked','--manifest-path',str(destination/'Cargo.toml'),'--target-dir',str(current/'target'),'authority_tests::native_','--','--nocapture']
    result=subprocess.run(command,text=True,capture_output=True,timeout=300)
    output=result.stdout+'\n'+result.stderr
    expected=['native_old_open_admission_cannot_clear_replacement_session','native_delayed_expected_disconnect_cannot_clear_replacement_session','native_stale_tls_completion_cannot_publish_for_a_new_admission']
    markers=['CAUSAL_RED old-open-erased-new-session','CAUSAL_RED old-disconnect-erased-new-session','CAUSAL_RED stale-tls-granted-new-generation-before-its-open']
    if not all(marker in output for marker in markers) or result.returncode!=101 or not all(re.search(r'test authority_tests::'+name+r' \.\.\. FAILED',output) for name in expected) or '0 passed; 3 failed;' not in output:
        print('Historical native RED gate mismatch:', {'exit':result.returncode,'compilerCodes':re.findall(r'error\[E[0-9]{4}\]',output),'causalMarkers':[marker in output for marker in markers]})
        # Source/test diagnostics contain no credentials; do not print captured
        # subprocess streams or fixture paths merely to make this gate pass.
        raise SystemExit(1)
    print('PASS historical exact-b2cb + scheduling-only hooks: all3 ACTUAL native/TLS authority regressions are RED')
