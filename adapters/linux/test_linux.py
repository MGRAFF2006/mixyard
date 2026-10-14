"""Offline command/graph-model tests. These do not qualify native audio."""

from copy import deepcopy
import io
import json
import os
from pathlib import Path
import signal
import select
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

import mixyard_linux as m

HERE = Path(__file__).parent


def obj(oid, what, **p):
    return {"id": oid, "type": "PipeWire:Interface:" + what, "info": {"props": p}}


def source_graph(channels=("MONO",)):
    return [obj(10, "Node", **{"node.name": "mixyard-test-mic", "media.class": "Audio/Source"})] + [
        obj(11+i, "Port", **{"node.id": 10, "port.direction": "out", "audio.channel": channel,
                            "object.serial": 100+i}) for i, channel in enumerate(channels)]


def sink_graph(name="headphones", oid=30):
    return [obj(oid, "Node", **{"node.name": name, "media.class": "Audio/Sink"})] + [
        obj(oid+1+i, "Port", **{"node.id": oid, "port.direction": "in", "audio.channel": c})
        for i, c in enumerate(("FL", "FR"))]


def consent_tokens(config):
    return [path["token"] for path in m.required_mic_consents(config)]


def evaluate(module, samples):
    """Interpret only the emitted native copy/mixer graph as scalar math."""
    graph = module["args"]["filter.graph"]
    values = {name.replace(":In", ":Out"): sample for name, sample in zip(graph["inputs"], samples)}
    for node in graph["nodes"]:
        if node["label"] != "mixer":
            continue
        total = 0
        for link in graph["links"]:
            if link["input"].startswith(node["name"] + ":"):
                index = link["input"].split("In ")[1]
                total += values[link["output"]] * node["control"]["Gain " + index]
        values[node["name"] + ":Out"] = total
    return [values[output] for output in graph["outputs"]]


def mix_modules(planned):
    return [mod for mod in planned["producerConfig"]["context.modules"]
            if ".mix." in mod.get("args", {}).get("capture.props", {}).get("node.name", "")]



class LinuxOffline(unittest.TestCase):
    def setUp(self):
        self.config = json.loads((HERE / "example.json").read_text())

    def test_independent_sends_channel_mapping_and_stable_topology(self):
        planned = m.plan(self.config, source_graph())
        modules = mix_modules(planned)
        self.assertEqual([evaluate(mod, [0.2]) for mod in modules], [[0.2], [0.1, 0.1], [0, 0]])
        self.assertEqual(len(planned["routes"]), 2)
        changed = deepcopy(self.config)
        changed["sources"][0]["gains"]["record"] = 0.25
        after = m.plan(changed, source_graph())
        self.assertEqual(planned["routes"], after["routes"])
        self.assertEqual(m.topology(planned["producerConfig"]), m.topology(after["producerConfig"]))
        self.assertEqual(m.controls(planned["producerConfig"])["mixyard.mathis.mix.call.in"],
                         m.controls(after["producerConfig"])["mixyard.mathis.mix.call.in"])
        changed["sources"][0]["mute"] = True
        muted = m.plan(changed, source_graph())
        self.assertEqual(muted["routes"], planned["routes"])
        self.assertEqual(evaluate(mix_modules(muted)[0], [0.2]), [0])
        self.config["sources"][0]["channels"] = ["FL", "FR"]
        stereo = m.plan(self.config, source_graph(("FL", "FR")))
        self.assertEqual([evaluate(mod, [0.2, -0.1]) for mod in mix_modules(stereo)],
                         [[0.05], [0.1, -0.05], [0, 0]])
        self.assertNotIn("audio.rate", json.dumps(stereo))

    def test_no_default_missing_ambiguity_and_channel_mismatch(self):
        for graph in ([], source_graph() + [obj(90, "Node", **m.props(source_graph()[0]))],
                      source_graph(("FL", "FR"))):
            planned = m.plan(self.config, graph)
            self.assertEqual(planned["routes"], [])
            self.assertEqual(planned["diagnostics"][0]["code"], "source-disconnected")
        altered = deepcopy(self.config)
        altered["sources"][0]["selector"]["device.serial"] = "usb-a"
        graph = source_graph()
        m.props(graph[0])["device.id"] = 100
        graph.append(obj(100, "Device", **{"device.serial": "usb-a"}))
        self.assertEqual(len(m.plan(altered, graph)["routes"]), 2)
        m.props(graph[-1])["device.serial"] = "usb-b"
        self.assertEqual(m.plan(altered, graph)["routes"], [])

    def test_validation_feedback_and_headroom(self):
        for bad in (-1, float("nan"), float("inf"), 10**1000, True, "0.5"):
            config = deepcopy(self.config)
            config["sources"][0]["gains"]["call"] = bad
            with self.assertRaises(ValueError):
                m.validate(config)
        for transform in (
            lambda c: c["sources"][0].update(role="call-return", mute=True),
            lambda c: c["sources"][0].update(channels=["FL", "FR", "FC"]),
            lambda c: c["sources"][0]["selector"].update({"node.name": "mixyard.mathis.mix.record.out"}),
            lambda c: c["sources"][0]["selector"].update({"object.id": "10"}),
            lambda c: c["mixes"][0].update(master=1.1),
            lambda c: c["outputs"][0].pop("selector"),
        ):
            config = deepcopy(self.config)
            transform(config)
            with self.assertRaises(ValueError):
                m.validate(config)
        config = deepcopy(self.config)
        second = deepcopy(config["sources"][0])
        second.update(id="other", selector={"node.name": "other-mic"})
        config["sources"].append(second)
        with self.assertRaisesRegex(ValueError, "peak bound"):
            m.plan(config, [])  # Headroom holds even when every source is missing.
        config["mixes"][0]["master"] = 0.5
        m.validate(config)
        graph = source_graph()
        m.props(graph[0])[m.TAG] = "another-instance"
        with self.assertRaisesRegex(ValueError, "feedback"):
            m.plan(self.config, graph)
        graph = source_graph()
        m.props(graph[0])["bluez5.loopback"] = True
        self.assertEqual(m.plan(self.config, graph)["routes"], [])

    def test_monitor_requires_explicit_arm_and_sink(self):
        self.config["outputs"][0].update(enabled=True, selector={"node.name": "headphones"})
        self.config["sources"][0]["gains"]["monitor"] = 0.1
        graph = source_graph() + [obj(30, "Node", **{"node.name": "headphones", "media.class": "Audio/Sink"})]
        graph += [obj(31+i, "Port", **{"node.id": 30, "port.direction": "in", "audio.channel": c})
                  for i, c in enumerate(("FL", "FR"))]
        self.assertEqual(len(m.plan(self.config, graph)["routes"]), 2)
        unconsented = m.plan(self.config, graph, True)
        self.assertEqual(len(unconsented["routes"]), 2)
        self.assertEqual(unconsented["diagnostics"][0]["code"], "mic-consent-required")
        tokens = consent_tokens(self.config)
        self.assertEqual(len(m.plan(self.config, graph, True, tokens)["routes"]), 7)
        missing = m.plan(self.config, source_graph(), True, tokens)
        self.assertEqual(missing["diagnostics"][0]["code"], "output-disconnected")
        for mod in mix_modules(missing):
            for side in ("capture.props", "playback.props"):
                self.assertFalse(mod["args"][side]["node.autoconnect"])
                self.assertTrue(mod["args"][side]["node.dont-fallback"])

    def test_custom_mix_ids_names_count_and_exclusions(self):
        self.config["mixes"][0].update(id="team_chat", name="Besprechung 🎧")
        send = self.config["sources"][0]["gains"].pop("call")
        self.config["sources"][0]["gains"]["team_chat"] = send
        for index in range(9):
            self.config["mixes"].append({"id": f"custom_{index}", "name": f"Custom {index}",
                "channels": ["MONO"], "master": 1, "virtualInput": True, "excludeRoles": ["call-return"]})
        planned = m.plan(self.config, source_graph())
        self.assertEqual(len(mix_modules(planned)), 12)
        first = mix_modules(planned)[0]["args"]["playback.props"]
        self.assertEqual(first["node.description"], "Mixyard Besprechung 🎧")
        self.assertEqual(first["node.name"], "mixyard.mathis.mix.team_chat.out")
        self.config["sources"][0]["role"] = "call-return"
        with self.assertRaisesRegex(ValueError, "excluded source"):
            m.plan(self.config, source_graph())
        self.config["sources"][0]["gains"]["team_chat"] = 0
        self.assertFalse(any(r["to"].endswith("team_chat.in") for r in m.plan(self.config, source_graph())["routes"]))

    def test_one_custom_mix_fanout_independent_output_gains(self):
        self.config["mixes"][1].update(id="desk_mix", name="Desk mix")
        self.config["sources"][0]["gains"]["desk_mix"] = self.config["sources"][0]["gains"].pop("record")
        self.config["outputs"] = [{"id": f"device_{i}", "mix": "desk_mix", "gain": value,
            "selector": {"node.name": f"sink-{i}"}, "channels": ["FL", "FR"], "enabled": True}
            for i, value in enumerate((1.0, 0.25))]
        graph = source_graph()
        for index in range(2):
            oid = 30 + index * 10
            graph.append(obj(oid, "Node", **{"node.name": f"sink-{index}", "media.class": "Audio/Sink"}))
            graph += [obj(oid+1+i, "Port", **{"node.id": oid, "port.direction": "in", "audio.channel": c})
                      for i, c in enumerate(("FL", "FR"))]
        before = m.plan(self.config, graph, True, consent_tokens(self.config))
        modules = before["producerConfig"]["context.modules"][-2:]
        self.assertEqual([evaluate(mod, [0.1, -0.1]) for mod in modules], [[0.1, -0.1], [0.025, -0.025]])
        self.assertEqual(len(before["routes"]), 10)
        self.config["outputs"][0]["gain"] = 0.5
        after = m.plan(self.config, graph, True, consent_tokens(self.config))
        self.assertEqual(before["routes"], after["routes"])
        self.assertEqual(m.topology(before["producerConfig"]), m.topology(after["producerConfig"]))
        self.assertEqual(m.controls(before["producerConfig"])["mixyard.mathis.output.device_1.in"],
                         m.controls(after["producerConfig"])["mixyard.mathis.output.device_1.in"])
        for output in self.config["outputs"]:
            output.update(gain=1, selector={"node.name": "sink-0"})
        third = dict(self.config["outputs"][0], id="third")
        self.config["outputs"].append(third)
        with self.assertRaisesRegex(ValueError, "same configured sink"):
            m.validate(self.config)

    def test_native_mixer_cascade_avoids_eight_source_product_limit(self):
        source = self.config["sources"][0]
        self.config["sources"] = [dict(source, id=f"mic_{i}", selector={"node.name": f"source-{i}"},
            gains={"call": 1/16, "record": 0.5/16, "monitor": 0}) for i in range(16)]
        modules = mix_modules(m.plan(self.config, []))
        for observed, expected in zip([evaluate(mod, [0.2]*16) for mod in modules], [[0.2], [0.1, 0.1], [0, 0]]):
            for value, target in zip(observed, expected):
                self.assertAlmostEqual(value, target)
        for module in modules:
            self.assertTrue(all(len(n.get("control", {})) <= 8 for n in module["args"]["filter.graph"]["nodes"]))

    def test_snapshot_envelope_error_and_privacy(self):
        with patch.object(m, "dump_graph", side_effect=FileNotFoundError("pw-dump")):
            result, code = m.discover("unused")
        self.assertEqual(code, 1)
        self.assertEqual(result["schemaVersion"], 1)
        self.assertEqual(result["diagnostics"][0]["code"], "dependency-unavailable")
        self.assertFalse(any(result["capabilities"].values()))
        with patch.object(m, "dump_graph", side_effect=PermissionError("denied")):
            result, code = m.discover("unused")
        self.assertEqual((code, result["diagnostics"][0]["code"]), (1, "permission-denied"))
        graph = source_graph()
        m.props(graph[0])["application.process.cmdline"] = "private data"
        with patch.object(m, "dump_graph", return_value=graph):
            result, code = m.discover("unused")
        self.assertEqual(code, 0)
        self.assertTrue(result["capabilities"]["deviceDiscovery"])
        self.assertFalse(result["capabilities"]["virtualInputs"])
        self.assertNotIn("private data", json.dumps(result))
        self.assertIn("xrun-counters-unavailable", json.dumps(result))

    def test_mutation_disabled_before_any_external_access(self):
        # No command status, stale config, ID reuse or repeated call can enable mutation.
        with patch.object(m.subprocess, "run") as run, patch.object(m, "spawn") as spawn, \
                patch.object(Path, "read_text") as read:
            for remote in ("pipewire-0", "mixyard-test-offline"):
                for armed in (False, True):
                    for _ in range(5):
                        with self.assertRaisesRegex(ValueError, "external graph mutation unavailable"):
                            m.apply(Path("missing.json"), remote, "/missing", armed, 0.1)
            run.assert_not_called()
            spawn.assert_not_called()
            read.assert_not_called()
        with patch.object(sys, "argv", ["mixyard-linux", "apply", "missing.json",
                "--remote", "mixyard-test-offline", "--runtime-dir", "/missing", "--arm-outputs"]), \
                patch.object(m.subprocess, "run") as run, patch.object(m, "spawn") as spawn, \
                patch("sys.stderr", new_callable=io.StringIO) as stderr:
            self.assertEqual(m.main(), 1)
            self.assertIn("no override", stderr.getvalue())
            run.assert_not_called()
            spawn.assert_not_called()

    def test_mic_consent_is_per_path_and_invalidated_by_edits(self):
        self.config["outputs"] = [dict(self.config["outputs"][0], id=f"out_{i}", mix="record",
            enabled=True, selector={"node.name": f"sink-{i}"}) for i in range(2)]
        graph = source_graph() + sink_graph("sink-0", 30) + sink_graph("sink-1", 40)
        paths = m.required_mic_consents(self.config)
        one = m.plan(self.config, graph, True, [paths[0]["token"]])
        self.assertTrue(any(r["to"] == {"node.name": "sink-0"} for r in one["routes"]))
        self.assertFalse(any(r["to"] == {"node.name": "sink-1"} for r in one["routes"]))
        self.assertEqual(one["diagnostics"][0]["code"], "mic-consent-required")
        self.assertFalse(one["mutationAvailable"])
        tokens = consent_tokens(self.config)
        for edit in (
            lambda c: c["sources"][0]["gains"].update(record=0.25),
            lambda c: c["sources"][0]["selector"].update({"device.serial": "replacement"}),
            lambda c: c["sources"][0].update(mute=True),
            lambda c: c["mixes"][1].update(master=0.5),
            lambda c: c["outputs"][0].update(gain=0.25),
            lambda c: c["outputs"][0]["selector"].update({"device.serial": "replacement"}),
        ):
            config = deepcopy(self.config)
            edit(config)
            planned = m.plan(config, graph, True, tokens)
            self.assertFalse(any(r["to"] == config["outputs"][0]["selector"] for r in planned["routes"]))
            self.assertTrue(any(d["code"] == "mic-consent-required" for d in planned["diagnostics"]))
        # Muted/zero-master paths still need consent, so unmuting cannot inherit consent.
        self.config["sources"][0]["mute"] = True
        self.config["mixes"][1]["master"] = 0
        self.assertEqual(len(m.required_mic_consents(self.config)), 2)
        muted_tokens = consent_tokens(self.config)
        self.config["sources"][0]["mute"] = False
        self.assertTrue(set(muted_tokens).isdisjoint(consent_tokens(self.config)))
        # With two microphones in one path, consenting to only one keeps the sink disconnected.
        self.config["mixes"][1]["master"] = 1
        other = deepcopy(self.config["sources"][0])
        other.update(id="other", selector={"node.name": "other"}, gains={"record": 0.25})
        self.config["sources"].append(other)
        paths = m.required_mic_consents(self.config)
        only_first_mic = [p["token"] for p in paths if p["source"] == "mic"]
        planned = m.plan(self.config, graph, True, only_first_mic)
        self.assertFalse(any(isinstance(r["to"], dict) for r in planned["routes"]))

    def test_destination_convergence_rejected_independent_of_peak_or_arming(self):
        self.config["outputs"] = [dict(self.config["outputs"][0], id=f"out_{i}", mix="record",
            enabled=True, gain=g, selector={"node.name": "headphones"}) for i, g in enumerate((1, 0.5))]
        # Combined peak is only 0.75, but configured convergence is still rejected.
        with self.assertRaisesRegex(ValueError, "same configured sink"):
            m.plan(self.config, source_graph())
        self.config["outputs"][1]["selector"]["device.serial"] = "same-device"
        graph = source_graph() + sink_graph()
        m.props(graph[2])["device.serial"] = "same-device"
        for armed in (False, True):
            for g in (0, 0.5):
                self.config["outputs"][1]["gain"] = g
                with self.assertRaisesRegex(ValueError, "same sink"):
                    m.plan(self.config, graph, armed, consent_tokens(self.config))
        # A disabled output does not converge.
        self.config["outputs"][1]["enabled"] = False
        m.plan(self.config, graph, True, consent_tokens(self.config))

    def test_health_reports_native_errors_without_claiming_zero_dropouts(self):
        table = ("S ID QUANT RATE WAIT BUSY W/Q B/Q ERR FORMAT NAME\n"
                 "R 55 256 48000 1us 1us 0.1 0.1 2 F32P 1 48000 + mixyard.mathis.mix.call.out\n"
                 "R 56 256 48000 1us 1us 0.1 0.1 0 F32P 2 48000 private-app\n")
        observed = subprocess.CompletedProcess([], 0, stdout=table)
        with patch.object(m.subprocess, "run", return_value=observed):
            result, code = m.health("unused", 3)
        self.assertEqual(code, 0)
        self.assertEqual(result["ownedRows"][0]["errorsAndXruns"], 2)
        self.assertIsNone(result["zeroDropouts"])
        self.assertFalse(result["gapsMeasured"])
        self.assertNotIn("private-app", json.dumps(result))
        with patch.object(m.subprocess, "run", side_effect=FileNotFoundError("pw-top")):
            result, code = m.health("unused", 3)
        self.assertEqual(code, 1)
        self.assertFalse(result["countersAvailable"])

    def test_parent_death_releases_child_process_not_audio_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            pidfile = Path(directory) / "child.pid"
            code = """
import os, sys, time
from pathlib import Path
import mixyard_linux as m
p = m.spawn([sys.executable, '-c', 'import time; time.sleep(30)'], dict(os.environ))
pidfile = Path(sys.argv[1])
staged = pidfile.with_suffix('.tmp')
with staged.open('w') as stream:
    print('staged', flush=True)
    sys.stdin.readline()
    stream.write(str(p.pid))
staged.replace(pidfile)
print('published', flush=True)
time.sleep(30)
"""
            parent = subprocess.Popen([sys.executable, "-B", "-c", code, str(pidfile)], cwd=HERE,
                                      stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)

            def handshake(expected):
                self.assertTrue(select.select([parent.stdout], [], [], 3)[0], "PID publisher not ready")
                self.assertEqual(parent.stdout.readline().strip(), expected)

            try:
                handshake("staged")
                self.assertTrue(pidfile.with_suffix(".tmp").exists())
                self.assertFalse(pidfile.exists())  # An incomplete write is never public.
                parent.stdin.write("publish\n")
                parent.stdin.flush()
                handshake("published")
                child_pid = int(pidfile.read_text())
                parent.kill()
                parent.wait(timeout=3)
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline:
                    status = Path(f"/proc/{child_pid}/stat")
                    try:
                        stopped = status.read_text().split()[2] == "Z"
                    except (FileNotFoundError, ProcessLookupError):
                        stopped = True
                    if stopped:
                        break
                    time.sleep(0.01)
                else:
                    os.kill(child_pid, signal.SIGTERM)
                    self.fail("child survived controller SIGKILL")
            finally:
                if parent.poll() is None:
                    parent.kill()
                parent.wait()
                parent.stdin.close()
                parent.stdout.close()


if __name__ == "__main__":
    unittest.main()
