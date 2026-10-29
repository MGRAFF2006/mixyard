"""Small std-only setup regressions; no codec compilation or native dependencies."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest

from linux_library import resolve_directory


class InstalledArtifactTests(unittest.TestCase):
    def test_package_files_override_incorrect_metadata_directory(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            multiarch = root / "usr/lib/x86_64-linux-gnu"
            multiarch.mkdir(parents=True)
            for name in ["libzstd.so", "libzstd.a"]:
                (multiarch / name).write_bytes(b"selected")
            files = [str(path) for path in root.rglob("*")]
            for name in ["libzstd.so", "libzstd.a"]:
                self.assertEqual(resolve_directory(files, name), multiarch)
            with self.assertRaises(ValueError):
                resolve_directory(files, "libopus.so")

    def test_missing_mode_and_ambiguous_installations_fail(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            files = []
            for name in ["one", "two"]:
                directory = root / name
                directory.mkdir()
                path = directory / "libzstd.so"
                path.write_bytes(b"selected")
                files.append(str(path))
            with self.assertRaises(ValueError):
                resolve_directory(files, "libzstd.so")
            with self.assertRaises(ValueError):
                resolve_directory(files, "libzstd.a")


class LinkSelectionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory(prefix="mixyard-codec-link-setup-")
        cls.binary = Path(cls.temp.name) / "setup"
        source = Path(__file__).resolve().parents[1] / "build.rs"
        subprocess.run(["rustc", "--edition=2024", str(source), "-Dwarnings",
                        "-Cdebuginfo=0", "-Copt-level=s", "-Cstrip=symbols",
                        "-o", str(cls.binary)], check=True, timeout=60)

    @classmethod
    def tearDownClass(cls):
        cls.temp.cleanup()

    def invoke(self, output, a, b, kind="static", os_name="linux", target_env=""):
        env = {key: value for key, value in os.environ.items()
               if not key.startswith(("MIXYARD_", "CARGO_FEATURE_SYSTEM_"))}
        env.update(OUT_DIR=str(output), CARGO_CFG_TARGET_OS=os_name,
                   CARGO_CFG_TARGET_ENV=target_env, CARGO_FEATURE_SYSTEM_ZSTD="1",
                   CARGO_FEATURE_SYSTEM_OPUS="1", MIXYARD_ZSTD_LIB_DIR=str(a),
                   MIXYARD_OPUS_LIB_DIR=str(b), MIXYARD_ZSTD_LINK_KIND=kind,
                   MIXYARD_OPUS_LINK_KIND=kind)
        return subprocess.run([str(self.binary)], env=env, capture_output=True,
                              text=True, timeout=10)

    def test_exact_artifacts_cannot_be_shadowed_by_other_codec_directory(self):
        for os_name, target_env, kind, names in [
            ("linux", "", "static", ("libzstd.a", "libopus.a")),
            ("linux", "", "dylib", ("libzstd.so", "libopus.so")),
            ("macos", "", "dylib", ("libzstd.dylib", "libopus.dylib")),
            ("windows", "msvc", "dylib", ("zstd.lib", "opus.lib")),
        ]:
            with self.subTest(os=os_name, kind=kind), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                a, b, out = (root / name for name in ["a", "b", "out"])
                for directory in [a, b, out]:
                    directory.mkdir()
                for directory in [a, b]:
                    for name in names:
                        (directory / name).write_bytes(b"wrong candidate")
                (a / names[0]).write_bytes(b"exact zstd")
                (b / names[1]).write_bytes(b"exact opus")
                result = self.invoke(out, a, b, kind, os_name, target_env)
                self.assertEqual(result.returncode, 0, result.stderr)
                paths = [line for line in result.stdout.splitlines()
                         if line.startswith("cargo:rustc-link-search=")]
                self.assertEqual(paths, [f"cargo:rustc-link-search=native={out}"] * 2)
                self.assertEqual(sorted(p.read_bytes() for p in out.iterdir()),
                                 [b"exact opus", b"exact zstd"])
                links = [line for line in result.stdout.splitlines()
                         if line.startswith("cargo:rustc-link-lib=")]
                self.assertEqual(len(links), 2)
                self.assertTrue(all("=mixyard_codec_" in line for line in links))
                for directory, selected in [(a, a / names[0]), (b, b / names[1])]:
                    for path in [directory, selected, selected.resolve()]:
                        self.assertIn(f"cargo:rerun-if-changed={path}", result.stdout)
                # Previously staged bytes must not let a removed selection pass.
                (b / names[1]).unlink()
                missing = self.invoke(out, a, b, kind, os_name, target_env)
                self.assertNotEqual(missing.returncode, 0)
                self.assertIn("missing selected library", missing.stderr)

    def test_symlink_target_and_changed_contents_are_tracked(self):
        if os.name == "nt":
            self.skipTest("symlink fixture requires Unix; other selection fixtures run on Windows")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            a, b, out = (root / name for name in ["a", "b", "out"])
            for directory in [a, b, out]:
                directory.mkdir()
            target = a / "libzstd.so.1"
            target.write_bytes(b"first")
            (a / "libzstd.so").symlink_to(target.name)
            (b / "libopus.so").write_bytes(b"opus")
            first = self.invoke(out, a, b, "dylib")
            self.assertEqual(first.returncode, 0, first.stderr)
            self.assertIn(f"cargo:rerun-if-changed={target}", first.stdout)
            target.write_bytes(b"second")
            second = self.invoke(out, a, b, "dylib")
            self.assertEqual(second.returncode, 0, second.stderr)
            self.assertNotEqual(first.stdout, second.stdout)


if __name__ == "__main__":
    unittest.main()
