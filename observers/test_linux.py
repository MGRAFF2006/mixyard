"""Synthetic passive observer fixtures; no native audio or GUI access."""
import datetime
import errno
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("observer", Path(__file__).with_name("linux.py"))
observer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(observer)
ADAPTER = Path(__file__).resolve().parents[1] / "adapters/linux/mixyard_linux.py"


def item(facts, name):
    return next(f for f in facts if f["key"]["name"] == name)


def node(oid=1, name="exact-node", serial="fixture-device"):
    return {"id": oid, "type": "PipeWire:Interface:Node", "info": {"state": "running", "props": {
        "node.name": name, "device.serial": serial, "media.class": "Audio/Sink"}}}


class Applications(unittest.TestCase):
    def process(self, root, pid, uid, executable):
        path = root / str(pid)
        path.mkdir()
        (path / "status").write_text(f"Name:\tignored\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n")
        (path / "exe").symlink_to(executable)
        return path

    def test_exact_mapping_current_user_only(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            self.process(root, 1, 101, "/opt/app")
            self.process(root, 2, 101, "/other/app")
            self.process(root, 3, 102, "/opt/other")
            self.process(root, 4, 101, "/opt/app")
            facts = observer.applications(root, 101, {"/opt/app": "org.example.Player", "/opt/other": "org.other"})
            self.assertEqual(facts[0]["value"], observer.value("text-set", ["org.example.Player"]))
            self.assertNotIn("ignored", json.dumps(facts))

    def test_empty_only_when_enumeration_complete(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            self.assertEqual(observer.applications(root, 101, {"/opt/app": "id"})[0]["value"], observer.value("text-set", []))
            self.assertIsNone(observer.applications(root, 101, {})[0]["value"])
            self.assertNotEqual(observer.applications(root / "missing", 101, {"/app": "id"})[0]["status"], "known")
            self.process(root, 1, 101, "/app")
            (root / "1/status").write_text("Uid: malformed")
            self.assertIsNone(observer.applications(root, 101, {"/app": "id"})[0]["value"])

    def test_permission_race_and_enumeration_limit_are_unknown(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            self.process(root, 1, 101, "/app")
            with patch.object(observer.os, "readlink", side_effect=PermissionError):
                self.assertEqual(observer.applications(root, 101, {"/app": "id"})[0]["status"], "unknown")
            with patch.object(observer.os, "readlink", side_effect=FileNotFoundError):
                self.assertIsNone(observer.applications(root, 101, {"/app": "id"})[0]["value"])
            with patch.object(observer.os, "readlink", return_value="/app (deleted)"):
                self.assertIsNone(observer.applications(root, 101, {"/app": "id"})[0]["value"])
            with patch.object(observer, "MAX_ENTRIES", 0):
                self.assertIsNone(observer.applications(root, 101, {"/app": "id"})[0]["value"])

    def test_pid_directory_replacement_cannot_change_admitted_identity(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            self.process(root, 1, 101, "/allowed")
            original = observer.read_text
            replaced = False
            def replace_after_owner(path, *args, **kwargs):
                nonlocal replaced
                result = original(path, *args, **kwargs)
                if Path(path).name == "status" and not replaced:
                    replaced = True
                    (root / "1").rename(root / "old")
                    self.process(root, 1, 102, "/other")
                return result
            with patch.object(observer, "read_text", side_effect=replace_after_owner):
                observed = observer.applications(root, 101, {"/allowed":"mine", "/other":"not-mine"})[0]
            self.assertEqual(observed["status"], "unknown")
            self.assertIsNone(observed["value"])


class Power(unittest.TestCase):
    def supply(self, root, name, **fields):
        path = root / name
        path.mkdir()
        for key, text in fields.items():
            (path / key).write_text(text)
        return path

    def test_battery_and_external_power(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            battery = self.supply(root, "BAT", type="Battery", present="1", status="Discharging", capacity="42")
            self.assertEqual(item(observer.power(root), "power.source")["value"], observer.value("text", "battery"))
            self.supply(root, "AC", type="Mains", online="1")
            self.assertEqual(item(observer.power(root), "power.source")["value"], observer.value("text", "external"))
            self.assertEqual(item(observer.power(root), "power.battery-percent")["value"], observer.value("number", 42))
            (battery / "capacity").write_text("101")
            self.assertTrue(all(f["status"] == "error" for f in observer.power(root)))

    def test_absent_multiple_malformed_permission_and_limits(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            self.assertTrue(all(f["value"] is None for f in observer.power(root)))
            self.supply(root, "BAT0", type="Battery", capacity="30", status="Discharging")
            self.supply(root, "BAT1", type="Battery", capacity="70", status="Discharging")
            self.assertIsNone(item(observer.power(root), "power.battery-percent")["value"])
            with patch.object(observer, "read_text", side_effect=PermissionError):
                self.assertTrue(all(f["status"] == "unknown" for f in observer.power(root)))
            with patch.object(observer, "read_text", return_value="x" * 4097):
                self.assertTrue(all(f["value"] is None for f in observer.power(root)))
            with patch.object(observer, "entries", side_effect=ValueError):
                self.assertTrue(all(f["status"] == "error" for f in observer.power(root)))
            self.assertTrue(all(f["value"] is None for f in observer.power(root / "missing")))

    def test_file_reader_size_and_utf8_bounds(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "file"
            path.write_bytes(b"x" * 4097)
            with self.assertRaises(ValueError):
                observer.read_text(path)
            path.write_bytes(b"\xff")
            with self.assertRaises(ValueError):
                observer.read_text(path)


class Civil(unittest.TestCase):
    def facts(self, utc):
        return observer.civil("Europe/Berlin", datetime.datetime.fromisoformat(utc).timestamp())

    def test_spring_gap_and_autumn_repeated_hour(self):
        before = self.facts("2026-03-29T00:59:00+00:00")
        after = self.facts("2026-03-29T01:00:00+00:00")
        self.assertEqual(item(before, "civil.minute-of-day")["value"]["value"], 119)
        self.assertEqual(item(after, "civil.minute-of-day")["value"]["value"], 180)
        for utc in ["2026-10-25T00:30:00+00:00", "2026-10-25T01:30:00+00:00"]:
            facts = self.facts(utc)
            self.assertEqual(item(facts, "civil.minute-of-day")["value"]["value"], 150)
            self.assertEqual(item(facts, "civil.weekday")["value"]["value"], 7)
        self.assertTrue(all(f["status"] == "error" for f in observer.civil("Not/AZone", 0)))

    def test_civil_date_uses_configured_zone(self):
        facts = self.facts("2026-10-27T23:30:00+00:00")
        self.assertEqual(item(facts, "civil.date")["value"]["value"], "2026-10-28")
        self.assertEqual(item(facts, "civil.timezone")["value"]["value"], "Europe/Berlin")


class Audio(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.adapter = observer.load_adapter(ADAPTER)

    def test_identity_stable_exact_opaque_and_declarations_not_inferred(self):
        first = observer.audio_objects([node()], self.adapter)
        same = node(99)
        same["info"]["props"]["node.description"] = "renamed label"
        self.assertEqual(first, observer.audio_objects([same], self.adapter))
        uid = first[0]["value"]["value"][0]
        self.assertTrue(uid.startswith("context:pipewire:"))
        self.assertNotIn("exact-node", uid)
        other = observer.audio_objects([node(name="Exact-node")], self.adapter)
        self.assertNotEqual(uid, other[0]["value"]["value"][0])
        self.assertEqual(item(first, "device.hardware-monitor-declared")["status"], "unsupported")
        self.assertIsNone(item(first, "device.channels")["value"])

    def test_duplicate_ambiguous_and_incomplete_not_empty(self):
        facts = observer.audio_objects([node(), node(2)], self.adapter)
        self.assertEqual(facts[0]["status"], "ambiguous")
        self.assertIsNone(facts[0]["value"])
        missing = node()
        del missing["info"]["props"]["node.name"]
        self.assertEqual(observer.audio_objects([missing], self.adapter)[0]["status"], "unknown")
        dangling = node()
        dangling["info"]["props"]["device.id"] = "99"
        self.assertEqual(observer.audio_objects([dangling], self.adapter)[0]["status"], "unknown")
        self.assertEqual(observer.audio_objects([], self.adapter)[0]["value"], observer.value("text-set", []))
        with patch.object(observer, "MAX_DEVICES", 0):
            with self.assertRaises(ValueError):
                observer.audio_objects([node()], self.adapter)
        with self.assertRaises(ValueError):
            observer.audio_objects([node(), node()], self.adapter)
        bad = node()
        bad["info"]["props"]["node.name"] = []
        with self.assertRaises(ValueError):
            observer.audio_objects([bad], self.adapter)

    def test_missing_permissions_malformed_oversized_do_not_prove_disconnection(self):
        for failure in [FileNotFoundError(), PermissionError(), TimeoutError(), OSError(errno.EIO, "fixture")]:
            with patch.object(observer, "bounded_command", side_effect=failure):
                facts = observer.audio(ADAPTER)
                self.assertNotEqual(facts[0]["status"], "known")
                self.assertIsNone(facts[0]["value"])
        for raw in [b"{}", b"[null]", b"not JSON", b"[{\"id\":1,\"id\":2}]", b"\xff"]:
            with patch.object(observer, "bounded_command", return_value=raw):
                self.assertEqual(observer.audio(ADAPTER)[0]["status"], "error")
        with patch.object(observer, "bounded_command", side_effect=ValueError("output-limit")):
            self.assertEqual(observer.audio(ADAPTER)[0]["status"], "error")

    def test_unclassifiable_nodes_do_not_prove_empty_endpoint_inventory(self):
        partial = node()
        del partial["info"]["props"]["media.class"]
        for objects in [[{"id":1,"type":"PipeWire:Interface:Node"}], [partial]]:
            with patch.object(observer, "bounded_command", return_value=json.dumps(objects).encode()):
                observed = observer.audio(ADAPTER)[0]
            self.assertNotEqual(observed["status"], "known")
            self.assertIsNone(observed["value"])
        unrelated = node()
        unrelated["info"]["props"]["media.class"] = "Video/Source"
        self.assertEqual(observer.audio_objects([unrelated], self.adapter)[0]["value"], observer.value("text-set", []))


@unittest.skipUnless(sys.platform == "linux", "Linux selectors/pipe behavior")
class Commands(unittest.TestCase):
    def test_zero_exit_diagnostics_do_not_certify_complete_discovery(self):
        with self.assertRaises(OSError):
            observer.bounded_command([sys.executable, "-c", "import sys; print('[]'); sys.stderr.write('fixture bind failure')"])

    def test_success_exit_stderr_size_and_timeout_are_bounded(self):
        run = observer.bounded_command
        self.assertEqual(run([sys.executable, "-c", "print('fixture')"]), b"fixture\n")
        for code, expected in [("import time; time.sleep(5)", TimeoutError),
                               ("import sys; sys.exit(1)", OSError),
                               ("import sys; sys.stderr.write('x'*100000)", ValueError),
                               ("print('x'*100000)", ValueError)]:
            with self.assertRaises(expected):
                run([sys.executable, "-c", code], timeout=0.15, limit=1024)

    def test_timeout_reaps_direct_child(self):
        with tempfile.TemporaryDirectory() as temp:
            pidfile = Path(temp) / "pid"
            code = "import os,time,pathlib; pathlib.Path(%r).write_text(str(os.getpid())); time.sleep(5)" % str(pidfile)
            with self.assertRaises(TimeoutError):
                observer.bounded_command([sys.executable, "-c", code], timeout=0.2)
            pid = int(pidfile.read_text())
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)


if __name__ == "__main__":
    unittest.main()
