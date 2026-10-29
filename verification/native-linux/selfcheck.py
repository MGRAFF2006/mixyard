#!/usr/bin/env python3
"""Fixture-only negative tests of discovery evidence validation."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('qualification', Path(__file__).with_name('qualify.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class EvidenceValidation(unittest.TestCase):
    def test_rejects_incomplete_or_ambiguous_metadata(self):
        core = {'id': 0, 'type': 'PipeWire:Interface:Core'}
        for value in ({}, [], [core, core], [{'id': 1, 'type': 'PipeWire:Interface:Node'}],
                      [{'id': True, 'type': 'PipeWire:Interface:Core'}]):
            with self.subTest(value=value), self.assertRaises(ValueError):
                module.inventory_summary(value)

    def test_observed_objects_are_counted_without_identity_disclosure(self):
        self.assertEqual(module.inventory_summary([
            {'id': 0, 'type': 'PipeWire:Interface:Core'},
            {'id': 3, 'type': 'PipeWire:Interface:Node'}]),
            {'PipeWire:Interface:Core': 1, 'PipeWire:Interface:Node': 1})


if __name__ == '__main__':
    unittest.main()
