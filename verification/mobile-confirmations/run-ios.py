"""Run only on a disposable hosted Mac; create/delete one owned simulator."""
import json
import os
from pathlib import Path
import subprocess

assert os.environ.get('GITHUB_ACTIONS') == 'true' and os.environ.get('RUNNER_OS') == 'macOS'
runtimes=json.loads(subprocess.check_output(['xcrun','simctl','list','runtimes','--json']))['runtimes']
runtime=next(value['identifier'] for value in reversed(runtimes) if value.get('isAvailable') and 'iOS' in value['identifier'])
identifier=subprocess.check_output(['xcrun','simctl','create','mixyard-confirmations','com.apple.CoreSimulator.SimDeviceType.iPhone-16',runtime],text=True).strip()
try:
    subprocess.run(['xcrun','simctl','boot',identifier],check=True,timeout=30)
    subprocess.run(['xcrun','simctl','bootstatus',identifier,'-b'],check=True,timeout=180)
    output=Path(os.environ['RUNNER_TEMP'])/'mixyard-confirm-ios'
    subprocess.run(['xcodebuild','test','-project','apps/mobile/ios/Mixyard.xcodeproj','-scheme','MixyardConfirmations','-configuration','Debug','-destination',f'platform=iOS Simulator,id={identifier}','-derivedDataPath',str(output),'CODE_SIGNING_ALLOWED=NO'],check=True,timeout=900)
finally:
    try: subprocess.run(['xcrun','simctl','shutdown',identifier],check=False,timeout=30)
    finally: subprocess.run(['xcrun','simctl','delete',identifier],check=True,timeout=30)
