"""Strict fixture decoder and bounded native smoke check; no raw OS output logs.

The product runtime must enforce its own trust/process boundary. This independent
decoder verifies every field emitted by these two helpers, not runtime behavior.
"""
import copy
import json
import subprocess
import sys
import threading
import unittest

MAX_BYTES = 1024 * 1024
MAX_ITEMS = 1024


def pairs(items):
    result = {}
    for key, value in items:
        if key in result:
            raise ValueError("duplicate key")
        result[key] = value
    return result


def fields(value, keys):
    if type(value) is not dict or set(value) != set(keys.split()):
        raise ValueError("unexpected fields")


def identifier(value):
    if type(value) is not str or not value.strip() or "\0" in value or len(value.encode("utf-8")) > 4096:
        raise ValueError("invalid identifier")


def audio_format(value):
    if value is None:
        return
    fields(value, "channels sampleRateHz")
    if type(value["channels"]) is not int or value["channels"] not in (1, 2):
        raise ValueError("unsupported channels")
    if type(value["sampleRateHz"]) is not int or not 0 < value["sampleRateHz"] <= 2**32 - 1:
        raise ValueError("unsupported rate")


def decode(data, platform):
    if not 0 < len(data) <= MAX_BYTES:
        raise ValueError("result bound")
    value = json.loads(data.decode("utf-8"), object_pairs_hook=pairs,
                       parse_constant=lambda _: (_ for _ in ()).throw(ValueError("nonfinite")))
    fields(value, "schemaVersion revision platform evidence sources outputs mediaSessions limits")
    if type(value["schemaVersion"]) is not int or value["schemaVersion"] != 2:
        raise ValueError("schema")
    if type(value["revision"]) is not int or value["revision"] != 0:
        raise ValueError("revision")
    if value["platform"] != platform or platform not in ("windows", "macos") or value["evidence"] != "native-discovery":
        raise ValueError("platform/evidence")
    seen = set()
    for catalog, shape in (("sources", "observedId selector origin format"), ("outputs", "observedId target format")):
        if type(value[catalog]) is not list or len(value[catalog]) > MAX_ITEMS:
            raise ValueError("catalog bound")
        for item in value[catalog]:
            fields(item, shape)
            identifier(item["observedId"])
            if item["observedId"] in seen:
                raise ValueError("duplicate observation ID")
            seen.add(item["observedId"])
            audio_format(item["format"])
            if catalog == "sources":
                selector = item["selector"]
                if type(selector) is not dict:
                    raise ValueError("selector")
                kind = selector.get("kind")
                key = {"mic": "deviceUid", "app": "applicationId"}.get(kind)
                if key is None:
                    raise ValueError("unknown source kind")
                fields(selector, "kind " + key)
                identifier(selector[key])
                if item["origin"] not in ("external", "mixyard-producer"):
                    raise ValueError("origin")
            else:
                fields(item["target"], "kind deviceUid")
                if item["target"]["kind"] != "device":
                    raise ValueError("unproved target")
                identifier(item["target"]["deviceUid"])
    if value["mediaSessions"] != []:
        raise ValueError("unproved media sessions")
    fields(value["limits"], "maxExportedMixes maxOutputRoutes maxVirtualInputs")
    if any(limit is not None for limit in value["limits"].values()):
        raise ValueError("unproved capacity")
    return value


def run_bounded(binary, command):
    """Own one process, drain both pipes with a combined bound and deadline.

    Threads support anonymous pipes on Windows too. Never echo native output.
    """
    process = subprocess.Popen([binary, command], stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    outputs = [bytearray(), bytearray()]
    lock = threading.Lock()
    overflow = threading.Event()

    def read(pipe, index):
        try:
            while True:
                chunk = pipe.read(4096)
                if not chunk:
                    return
                with lock:
                    if sum(map(len, outputs)) + len(chunk) > MAX_BYTES:
                        overflow.set()
                        process.kill()
                        return
                    outputs[index].extend(chunk)
        finally:
            pipe.close()

    readers = [threading.Thread(target=read, args=(pipe, i), daemon=True)
               for i, pipe in enumerate((process.stdout, process.stderr))]
    for reader in readers:
        reader.start()
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
        raise ValueError("native snapshot deadline exceeded") from None
    finally:
        for reader in readers:
            reader.join(timeout=1)
    if overflow.is_set() or any(reader.is_alive() for reader in readers):
        raise ValueError("native snapshot output exceeded bound or pipe lifetime")
    return process.returncode, bytes(outputs[0]), bytes(outputs[1])


def smoke(binary, platform):
    code, out, err = run_bounded(binary, "snapshot")
    value = decode(out, platform)
    if code == 0 and not err:
        print("Read-only hosted snapshot decoded; sources=%d outputs=%d. Physical audio UNQUALIFIED."
              % (len(value["sources"]), len(value["outputs"])))
    elif code == 1 and err == b"native-inventory-unavailable\n":
        # GitHub runners may have no Windows Audio service/active endpoints.
        if value["sources"] or value["outputs"]:
            raise ValueError("unavailable snapshot must be empty")
        print("Read-only hosted snapshot UNAVAILABLE (scoped failure); no device/audio qualification.")
    else:
        raise ValueError("unrecognized native failure (private diagnostics suppressed)")
    code, out, err = run_bounded(binary, "capture")
    if code != 2 or out or err != b"native-inventory-invalid-arguments\n":
        raise ValueError("unexpected command accepted")


BASE = {"schemaVersion": 2, "revision": 0, "platform": "macos", "evidence": "native-discovery",
        "sources": [{"observedId": "s0", "selector": {"kind": "mic", "deviceUid": "opaque"},
                     "origin": "external", "format": None}],
        "outputs": [{"observedId": "o0", "target": {"kind": "device", "deviceUid": "output"},
                     "format": {"channels": 2, "sampleRateHz": 44100}}],
        "mediaSessions": [], "limits": {"maxExportedMixes": None, "maxOutputRoutes": None, "maxVirtualInputs": None}}


class ProtocolTests(unittest.TestCase):
    def decode_value(self, value):
        return decode(json.dumps(value, ensure_ascii=True).encode(), "macos")

    def test_duplicates_are_distinct_observations(self):
        value = copy.deepcopy(BASE)
        second = copy.deepcopy(value["sources"][0])
        second["observedId"] = "s1"
        value["sources"].append(second)
        self.assertEqual(len(self.decode_value(value)["sources"]), 2)
        second["observedId"] = "s0"
        with self.assertRaises(ValueError):
            self.decode_value(value)

    def test_strict_json_and_all_consumed_fields(self):
        malformed = [b'{"schemaVersion":2,"schemaVersion":2}', b'{}{}', b'\xff', b'{}', b'NaN']
        for data in malformed:
            with self.subTest(data=data), self.assertRaises(ValueError):
                decode(data, "macos")
        paths = [(key,) for key in BASE]
        paths += [("sources", 0, key) for key in BASE["sources"][0]]
        paths += [("outputs", 0, key) for key in BASE["outputs"][0]]
        paths += [("sources", 0, "selector", key) for key in BASE["sources"][0]["selector"]]
        paths += [("outputs", 0, "target", key) for key in BASE["outputs"][0]["target"]]
        paths += [("outputs", 0, "format", key) for key in BASE["outputs"][0]["format"]]
        paths += [("limits", key) for key in BASE["limits"]]
        for path in paths:
            value = copy.deepcopy(BASE)
            parent = value
            for key in path[:-1]:
                parent = parent[key]
            del parent[path[-1]]
            with self.subTest(path=path), self.assertRaises(ValueError):
                self.decode_value(value)
        raw = json.dumps(BASE).replace('"deviceUid": "opaque"', '"deviceUid":"opaque","deviceUid":"other"')
        with self.assertRaises(ValueError):
            decode(raw.encode(), "macos")

    def test_bounds_and_unknown_values(self):
        changes = [("schemaVersion", True), ("revision", 1), ("platform", "windows"),
                   ("evidence", "simulated"), ("sources", [BASE["sources"][0]] * (MAX_ITEMS + 1)),
                   ("mediaSessions", [{}]), ("limits", {"maxExportedMixes": 2})]
        for key, replacement in changes:
            value = copy.deepcopy(BASE)
            value[key] = replacement
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.decode_value(value)
        for uid in ("", "\0", "\ud800", "a" * 4097):
            value = copy.deepcopy(BASE)
            value["sources"][0]["selector"]["deviceUid"] = uid
            with self.subTest(uid=uid[:10]), self.assertRaises(ValueError):
                self.decode_value(value)
        for fmt in ({"channels": 3, "sampleRateHz": 48000}, {"channels": 2, "sampleRateHz": 0},
                    {"channels": 2, "sampleRateHz": True}, {"channels": 2, "sampleRateHz": 48000, "encoding": "guess"}):
            value = copy.deepcopy(BASE)
            value["sources"][0]["format"] = fmt
            with self.subTest(fmt=fmt), self.assertRaises(ValueError):
                self.decode_value(value)
        with self.assertRaises(ValueError):
            decode(b" " * (MAX_BYTES + 1), "macos")


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--smoke":
        smoke(sys.argv[2], sys.argv[3])
    elif len(sys.argv) == 3 and sys.argv[1] == "--fixture":
        code, out, err = run_bounded(sys.argv[2], "fixture")
        if code or err:
            raise SystemExit("portable fixture failed")
        value = decode(out, "macos")
        assert len(value["sources"]) == 4 and len(value["outputs"]) == 1
        assert value["sources"][0]["selector"] == value["sources"][1]["selector"]
        assert value["sources"][0]["selector"]["deviceUid"] == 'opaque:"\\\nä🎧'
        assert value["sources"][0]["format"] == {"channels": 1, "sampleRateHz": 44100}
        assert value["sources"][1]["format"] is None
        print("Portable writer output decoded with exact opaque IDs, formats and duplicate selectors.")
    else:
        unittest.main()
