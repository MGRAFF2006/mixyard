"""Control-plane tests; these never import a native library or start PipeWire."""
import copy
import os
from pathlib import Path
import tempfile
import sys
import unittest
from unittest import mock
import qualify
from qualify import isolation


def object_(id_, kind, props=None, **info):
    return {"id": id_, "type": "PipeWire:Interface:" + kind,
            "info": {"props": props or {}, **info}}


def private_snapshot():
    from qualify import MODULES
    return [object_(0, "Core", {"core.name": "mixyard-private"}),
            object_(1, "Node", {"node.name": "mixyard-private-driver"})] + [
                object_(id_, "Module", {"module.name": name})
                for id_, name in enumerate(sorted(MODULES), 2)]


class Isolation(unittest.TestCase):
    def test_failed_bind_reports_dynamic_registry_but_never_qualifies(self):
        env = {"PIPEWIRE_REMOTE": "mixyard-private"}
        listing = b'id 42, type PipeWire:Interface:Metadata/3\n'
        with mock.patch.object(qualify, "snapshot_bytes", side_effect=[RuntimeError("owned bind error"), listing]) as capture:
            with self.assertRaisesRegex(RuntimeError, "owned bind error.*id 42.*Metadata/3"):
                qualify.snapshot(env)
            self.assertEqual(capture.call_args_list, [
                mock.call(["pw-dump", "--remote", "mixyard-private"], env),
                mock.call(["pw-cli", "--remote", "mixyard-private", "list-objects"], env)])
        with mock.patch.object(qualify, "snapshot_bytes", side_effect=[RuntimeError("owned bind error"), RuntimeError("registry unavailable")]):
            with self.assertRaisesRegex(RuntimeError, "owned bind error.*registry unavailable"):
                qualify.snapshot(env)

    def test_snapshot_capture_preserves_failure_cause_and_bounds_both_pipes(self):
        # Real owned Python control children, never PipeWire/native APIs.
        def capture(script, **kwargs):
            return qualify.snapshot_bytes([sys.executable, "-I", "-c", script], dict(os.environ), **kwargs)

        self.assertEqual(capture("import sys; sys.stdout.write('[]')"), b"[]")
        with self.assertRaisesRegex(RuntimeError, "stdoutBytes=2 stderrBytes=19.*owned-fixture-error"):
            capture("import sys; sys.stdout.write('[]'); sys.stderr.write('owned-fixture-error')")
        with self.assertRaisesRegex(RuntimeError, "returncode=3.*owned-exit-error"):
            capture("import sys; sys.stderr.write('owned-exit-error'); sys.exit(3)")
        with self.assertRaisesRegex(RuntimeError, "reason=stdout-limit.*stdoutBytes=1048577"):
            capture("import sys; sys.stdout.buffer.write(b'x' * (2 * 1024 * 1024))")
        with self.assertRaisesRegex(RuntimeError, "reason=stderr-limit.*stderrBytes=4097"):
            capture("import sys; sys.stderr.buffer.write(b'x' * 8192)")
        with self.assertRaisesRegex(RuntimeError, "reason=timeout"):
            capture("import time; time.sleep(10)", timeout=0.05)

    def test_daemon_launcher_selects_server_config_without_starting_native_code(self):
        class BeforeNativeLaunch(Exception):
            pass

        def inspect_launch(argv, *, env, **_):
            self.assertEqual(argv[0], "pipewire")
            self.assertEqual(argv[1], "-c")
            self.assertEqual(Path(argv[2]).name, "private.conf")
            # PipeWire gives this environment key precedence over -c. Check the
            # actual production launch boundary, not a parallel test launcher.
            self.assertEqual(env["PIPEWIRE_CONFIG_NAME"], "private.conf")
            config = Path(env["PIPEWIRE_CONFIG_DIR"])
            self.assertEqual(Path(argv[2]), config / env["PIPEWIRE_CONFIG_NAME"])
            self.assertIn("core.daemon = true", (config / "private.conf").read_text())
            self.assertIn("{ name = libpipewire-module-metadata }", (config / "private.conf").read_text())
            self.assertNotIn("core.daemon", (config / "client.conf").read_text())
            self.assertIn("{ name = libpipewire-module-metadata }", (config / "client.conf").read_text())
            self.assertEqual(env["PIPEWIRE_REMOTE"], "mixyard-private")
            raise BeforeNativeLaunch()

        with tempfile.TemporaryDirectory(prefix="mixyard-launch-check-", dir="/dev/shm") as root:
            environment = {"GITHUB_ACTIONS": "true", "RUNNER_ENVIRONMENT": "github-hosted",
                           "RUNNER_TEMP": root, "PIPEWIRE_CONFIG_NAME": "client.conf"}
            argv = [qualify.__file__, __file__, str(Path(root) / "evidence.json")]
            with mock.patch.dict(os.environ, environment), mock.patch("sys.argv", argv), \
                    mock.patch.object(qualify.subprocess, "Popen", side_effect=inspect_launch) as launch:
                with self.assertRaises(BeforeNativeLaunch):
                    qualify.main()
                launch.assert_called_once()
                self.assertEqual(os.environ["PIPEWIRE_CONFIG_NAME"], "client.conf")

    def test_pre_audio_gate_rejects_physical_nodes_extra_modules_and_links(self):
        accepted = private_snapshot()
        isolation(accepted)
        for contamination in (
            object_(50, "Device", {"device.api": "alsa"}),
            object_(50, "Node", {"node.name": "personal-speakers"}),
            object_(50, "Module", {"module.name": "libpipewire-module-protocol-pulse"}),
            object_(50, "Link"),
        ):
            with self.subTest(contamination=contamination), self.assertRaises(RuntimeError):
                isolation(accepted + [contamination])
        wrong_core = copy.deepcopy(accepted)
        wrong_core[0]["info"]["props"]["core.name"] = "pipewire-0"
        with self.assertRaises(RuntimeError):
            isolation(wrong_core)

    def test_active_graph_rejects_recapture_and_duplicate_consumer_links(self):
        objects = private_snapshot()
        names = ["mixyard-private-generator", "mixyard-private-engine", "mixyard-private-consumer"]
        objects += [object_(10+i, "Node", {"node.name": name}) for i, name in enumerate(names)]
        objects += [object_(30+i, "Link", **{"output-node-id": 10 if i < 3 else 11,
                                          "input-node-id": 11 if i < 3 else 12,
                                          "input-port-id": 40+i}) for i in range(6)]
        isolation(objects, active=True)
        recapture = copy.deepcopy(objects)
        recapture[-1]["info"]["input-node-id"] = 11
        with self.assertRaises(RuntimeError):
            isolation(recapture, active=True)
        duplicate = copy.deepcopy(objects)
        duplicate[-1]["info"]["input-port-id"] = 44
        with self.assertRaises(RuntimeError):
            isolation(duplicate, active=True)


if __name__ == "__main__":
    unittest.main()
