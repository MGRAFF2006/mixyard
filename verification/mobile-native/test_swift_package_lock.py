"""In-memory invalid-lock tests, NOT resolver provenance or a generated lock."""
import copy
import importlib.util
import json
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('trust_lock', Path(__file__).with_name('swift-package-lock.py'))
lock = importlib.util.module_from_spec(spec); spec.loader.exec_module(lock)


class MemoryDocument:
    def __init__(self, value): self.data = json.dumps(value).encode()
    def read_text(self): return self.data.decode()
    def read_bytes(self): return self.data


class SwiftLockAdmission(unittest.TestCase):
    def fixture(self):
        return {'version':3,'pins':[{'identity':'swift-asn1','kind':'remoteSourceControl',
                'location':lock.URL,'state':{'revision':lock.REVISION}}]}
    def test_only_exact_one_dependency_and_revision_are_admitted(self):
        # This synthetic input exercises the reader; it is never written into
        # Package.resolved, a build, a lock artifact or a provenance claim.
        self.assertEqual(len(lock.verify(MemoryDocument(self.fixture()))),64)
        for change in ('transitive','revision','branch','origin','kind','identity','version','schema','empty'):
            value=self.fixture(); pin=value['pins'][0]
            if change=='transitive':value['pins'].append(copy.deepcopy(pin))
            if change=='revision':pin['state']['revision']='0'*40
            if change=='branch':pin['state']['branch']='main'
            if change=='origin':pin['location']='https://unrelated.invalid/swift-asn1.git'
            if change=='kind':pin['kind']='localSourceControl'
            if change=='identity':pin['identity']='different'
            if change=='version':pin['state']['version']='1.7.3'
            if change=='schema':value['version']=1
            if change=='empty':value['pins']=[]
            with self.subTest(change=change),self.assertRaises(RuntimeError):lock.verify(MemoryDocument(value))
