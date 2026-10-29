"""File identity oracle regression; real loader/production runs belong to CI."""

import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


class RuntimeEvidenceTests(unittest.TestCase):
    def test_same_version_decoy_bytes_and_incomplete_probe_are_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            with patch.dict(os.environ, CARGO_TARGET_DIR=temp):
                spec = importlib.util.spec_from_file_location(
                    "qualification", Path(__file__).with_name("qualify.py"))
                module = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(module)
            selected = {codec: root / codec for codec in ["ZSTD", "OPUS"]}
            for path in selected.values():
                path.write_bytes(b"selected runtime version unchanged")
            observed = [f"codec_runtime_symbol={symbol}\t{selected[codec]}" for symbol, codec in [
                ("ZSTD_compress2", "ZSTD"), ("ZSTD_decompressDCtx", "ZSTD"),
                ("opus_encode_float", "OPUS"), ("opus_decode_float", "OPUS")]]
            output = "\n".join(observed)
            self.assertIn("sha256=", module.verify_loaded_bytes(output, selected))
            with self.assertRaisesRegex(RuntimeError, "missing actual runtime"):
                module.verify_loaded_bytes("\n".join(observed[:-1]), selected)
            with self.assertRaisesRegex(RuntimeError, "duplicate runtime"):
                module.verify_loaded_bytes(output + "\n" + observed[0], selected)
            decoy = root / "decoy"
            decoy.write_bytes(selected["OPUS"].read_bytes() + b"changed bytes")
            with self.assertRaisesRegex(RuntimeError, "runtime bytes differ from selected OPUS"):
                module.verify_loaded_bytes(output.replace(str(selected["OPUS"]), str(decoy)), selected)


if __name__ == "__main__":
    unittest.main()
