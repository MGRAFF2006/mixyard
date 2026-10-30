"""Control-plane tests; these never import a native library or start PipeWire."""
import copy
import os
import re
import signal
import time
import subprocess
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
                for id_, name in enumerate(sorted(MODULES), 2)] + [
                    object_(8, "Client", {"pipewire.access": "unrestricted"})]


class Isolation(unittest.TestCase):
    def test_actual_c_and_python_os_clock_share_the_recorded_namespace(self):
        base = Path(__file__).resolve().parent
        program = r'''#include <assert.h>
#include <inttypes.h>
#include <stdio.h>
#include "os_clock.h"
int main(void) {
    uint64_t ns, now;
    assert(mx_os_time_namespace(&ns) && mx_os_monotonic_ns(&now));
    printf("{\"namespace\":%" PRIu64 ",\"monotonicNs\":%" PRIu64 "}\n", ns, now);
    return 0;
}
'''
        with tempfile.TemporaryDirectory(prefix="mixyard-os-clock-", dir="/dev/shm") as root:
            source, binary = Path(root) / "clock.c", Path(root) / "clock"
            source.write_text(program)
            subprocess.run(["cc", "-std=c11", "-D_POSIX_C_SOURCE=200809L", "-Wall", "-Wextra", "-Werror",
                            "-pedantic", "-I", str(base), str(source), "-o", str(binary)],
                           env=dict(os.environ, TMPDIR=root), check=True, capture_output=True)
            before = time.clock_gettime_ns(time.CLOCK_MONOTONIC)
            observed = qualify.json.loads(subprocess.run([str(binary)], check=True, capture_output=True, text=True).stdout)
            after = time.clock_gettime_ns(time.CLOCK_MONOTONIC)
            self.assertEqual(observed["namespace"], os.stat("/proc/self/ns/time").st_ino)
            self.assertLessEqual(before, observed["monotonicNs"])
            self.assertLessEqual(observed["monotonicNs"], after)

    def test_stop_signal_mask_is_inherited_before_workers_and_acknowledged(self):
        base = Path(__file__).resolve().parent
        with tempfile.TemporaryDirectory(prefix="mixyard-stop-signals-", dir="/dev/shm") as root:
            binary = Path(root) / "stop-signals-check"
            subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", "-pedantic",
                            "-O2", "-g0", "-pthread", str(base / "stop_signals_check.c"),
                            "-o", str(binary)], env=dict(os.environ, TMPDIR=root),
                           check=True, capture_output=True)
            for name, number in [("INT", signal.SIGINT), ("TERM", signal.SIGTERM)]:
                with self.subTest(signal=name):
                    late = subprocess.run([str(binary), "late", name], capture_output=True,
                                          text=True, timeout=5)
                    self.assertEqual(late.returncode, -number, late.stdout + late.stderr)
                    self.assertEqual(qualify.json.loads(late.stdout), {
                        "workerIntBlocked": 0, "workerTermBlocked": 0, "ownerBlocked": True})
                    early = subprocess.run([str(binary), "early", name], capture_output=True,
                                           text=True, timeout=5, check=True)
                    observed, ack = map(qualify.json.loads, early.stdout.splitlines())
                    self.assertEqual(observed, {
                        "workerIntBlocked": 1, "workerTermBlocked": 1, "ownerBlocked": True})
                    self.assertEqual(ack, {"ownerAck": True, "workerJoined": True, "signal": number})
                    self.assertEqual(early.stderr, "")

    def test_owned_consumer_boundary_has_no_native_pointer_retention(self):
        base = Path(__file__).resolve().parent
        with tempfile.TemporaryDirectory(prefix="mixyard-owned-copy-", dir="/dev/shm") as root:
            binary = Path(root) / "owned-copy"
            subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", "-pedantic",
                            "-I", str(base.parents[1] / "engine/include"),
                            str(base / "consumer_copy_check.c"), "-o", str(binary)],
                           env=dict(os.environ, TMPDIR=root), check=True, capture_output=True)
            result = subprocess.run([str(binary)], check=True, capture_output=True, text=True)
            self.assertIn("full scrub passed", result.stdout)

    def test_first_native_failure_retains_exact_chunk_timeline_and_sample_bits(self):
        base = Path(__file__).resolve().parent
        program = r'''#include <assert.h>
#include "native_diagnostics.h"
int main(void) {
    struct mx_native_first_failure failure = {0}, empty = {0};
    struct mx_native_observation first = { .error = 12, .role = 2, .port = 1,
        .frames = 128, .chunk_present = true, .offset = 32, .size = 2048,
        .stride = 16, .flags = 1, .clock_id = 19, .clock_frame = 9007199254740993ULL,
        .header_present = true, .header_seq = 9007199254740865ULL, .header_pts = -7,
        .callback_monotonic_ns = 123456789, .sample_index = 3, .actual_bits = 0x8000000000000000ULL,
        .expected_bits = 0x3ff0000000000001ULL };
    mx_native_record_first(&failure, &first);
    struct mx_native_observation late = { .error = 11, .size = 0 };
    for (unsigned i = 0; i < 1024; ++i) mx_native_record_first(&failure, &late);
    assert(atomic_load(&failure.state) == 2);
    printf("{\"owned\":true"); mx_native_print_first_failure("failure", &failure);
    mx_native_print_first_failure("empty", &empty); puts("}");
    return 0;
}
'''
        with tempfile.TemporaryDirectory(prefix="mixyard-diagnostic-", dir="/dev/shm") as root:
            source, binary = Path(root) / "diagnostic.c", Path(root) / "diagnostic"
            source.write_text(program)
            subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", "-pedantic",
                            "-I", str(base), str(source), "-o", str(binary)],
                           env=dict(os.environ, TMPDIR=root), check=True, capture_output=True)
            result = qualify.json.loads(subprocess.run([str(binary)], check=True,
                capture_output=True, text=True).stdout)
        self.assertIsNone(result["empty"])
        self.assertEqual(result["failure"]["callbackMonotonicNs"], 123456789)
        report = result["failure"]
        self.assertEqual((report["error"], report["role"], report["port"]), (12, 2, 1))
        self.assertEqual((report["offset"], report["size"], report["stride"], report["flags"]),
                         (32, 2048, 16, 1))
        self.assertEqual((report["clockId"], report["clockFrame"], report["headerSeq"], report["headerPts"]),
                         (19, 9007199254740993, 9007199254740865, -7))
        self.assertEqual((report["actualBits"], report["expectedBits"], report["sampleIndex"]),
                         (0x8000000000000000, 0x3ff0000000000001, 3))

    def test_actual_c_module_guard_matches_server_and_python_isolation(self):
        base = Path(__file__).resolve().parent
        configured = re.findall(r'\{ name = (libpipewire-module-[\w-]+)',
                                (base / "private.conf").read_text())
        # Execute the same predicate/table used by global_added, without an SDK
        # or native context. Exact cross-language/config agreement is the rule.
        program = r'''#include <stdio.h>
#include "private_modules.h"
int main(void) {
    if (mx_private_module_allowed(NULL) || mx_private_module_allowed("") ||
        mx_private_module_allowed("libpipewire-module-protocol-pulse") ||
        mx_private_module_allowed("libpipewire-module-metadata-extra")) return 1;
    for (size_t i = 0; i < sizeof(mx_private_modules) / sizeof(mx_private_modules[0]); ++i) {
        if (!mx_private_module_allowed(mx_private_modules[i])) return 2;
        puts(mx_private_modules[i]);
    }
    return 0;
}
'''
        with tempfile.TemporaryDirectory(prefix="mixyard-module-guard-", dir="/dev/shm") as root:
            source = Path(root) / "guard.c"
            binary = Path(root) / "guard"
            source.write_text(program)
            subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", "-pedantic",
                            "-I", str(base), str(source), "-o", str(binary)],
                           env=dict(os.environ, TMPDIR=root), check=True, capture_output=True)
            actual = subprocess.run([str(binary)], check=True, capture_output=True, text=True).stdout.splitlines()
        self.assertEqual(len(actual), len(set(actual)))
        self.assertEqual(set(actual), set(configured))
        self.assertEqual(len(configured), len(actual))
        self.assertEqual(set(actual), qualify.MODULES)

    def test_private_access_mode_and_observed_client_classification(self):
        # Configuration representation is part of the pinned SDK policy: the
        # deprecated force key selected legacy mode but did not force a grant.
        # Keep that branch explicitly rather than changing socket/Flatpak rules.
        config = (Path(__file__).resolve().parent / "private.conf").read_text()
        entries = re.findall(r'\{ name = libpipewire-module-access args = \{([^}]+)\} \}', config)
        self.assertEqual(len(entries), 1)
        self.assertEqual(entries[0].split(), ["access.legacy", "=", "true"])
        for name in ("access.force", "access.allowed", "access.rejected", "access.restricted", "access.socket"):
            self.assertNotRegex(entries[0], re.escape(name))
        accepted = private_snapshot()
        self.assertEqual(isolation(accepted)["clientAccess"], [{"id": 8, "access": "unrestricted"}])
        for classification in (None, "default", "flatpak", "restricted"):
            wrong = copy.deepcopy(accepted)
            wrong[-1]["info"]["props"] = {} if classification is None else {"pipewire.access": classification}
            with self.subTest(access=classification), self.assertRaisesRegex(RuntimeError, "access classification"):
                isolation(wrong)
        with self.assertRaisesRegex(RuntimeError, "access observation is incomplete"):
            isolation(accepted[:-1])

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
