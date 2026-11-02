"""Build a canonical fixture policy bound to actually observed private ports.

Selectors here are synthetic policy aliases, never discovered physical app IDs.
Only the registry-bound observed IDs authorize native buffers in graph.c.
"""


def request(topology, mode="normal"):
    sources = topology["sources"]
    outputs = topology["outputs"]
    if len(sources) != 3 or len(outputs) != 3 or len(set(sources + outputs)) != 6:
        raise ValueError("three unique source and consumer ports required")
    selectors = [
        {"kind": "app", "applicationId": "owned-synthetic-policy-alias:" + sources[0]},
        {"kind": "call-return", "applicationId": "owned-synthetic-policy-alias:" + sources[1], "group": "owned-return"},
        {"kind": "app", "applicationId": "owned-synthetic-policy-alias:" + sources[2]},
    ]
    targets = [{"kind": "virtual-input", "endpointId": p} for p in outputs]
    names = ["a-program", "b-return", "c-private"]
    mix_names = ["a-call", "b-record", "c-monitor"]
    config = {
        "schemaVersion": 2, "revision": 1,
        "sources": [{"id": name, "selector": selector} for name, selector in zip(names, selectors)],
        "mixes": [{"id": name, "name": name, "channels": 2, "sampleRateHz": 48000,
                   "excludeGroups": ["owned-return"] if i == 0 else []}
                  for i, name in enumerate(mix_names)],
        "sends": [
            {"source": "a-program", "mix": "a-call", "gain": 1},
            {"source": "a-program", "mix": "b-record", "gain": 0.5},
            {"source": "b-return", "mix": "b-record", "gain": 0.25},
            {"source": "a-program", "mix": "c-monitor", "gain": 0.5},
        ],
        "outputs": [{"id": name, "mix": mix_name, "target": target, "gain": 1}
                    for name, mix_name, target in zip(["a-direct", "b-record", "c-monitor"], mix_names, targets)],
    }
    fmt = {"channels": 2, "sampleRateHz": 48000}
    inventory = {
        "schemaVersion": 2, "revision": 1, "platform": "linux",
        # PCM is real native transport; policy aliases/provenance are deliberately simulated.
        "evidence": "simulated",
        "sources": [{"observedId": p, "selector": s, "origin": "external", "format": fmt}
                    for p, s in zip(sources, selectors)],
        "outputs": [{"observedId": p, "target": t, "format": fmt} for p, t in zip(outputs, targets)],
    }
    port = lambda p: {"observedId": p, "sampleRateHz": 48000,
                      "channels": ["FL", "FR"], "mapping": ["FL", "FR"]}
    result = {"config": config, "inventory": inventory, "generation": 1,
            "clockId": topology["clockId"], "maxFrames": 128,
            "limits": {"maxSources": 3, "maxMixes": 3, "maxSends": 4, "maxOutputs": 3,
                       "maxBufferSamples": 8192, "maxTotalBytes": 1048576,
                       "maxDspWorkUnitsPerBlock": 1000000},
            "outputDsp": [None, {"name": "record-destination-only",
                "format": {"sampleRateHz": 48000, "channels": ["FL", "FR"]},
                "effects": [{"name": "explicit-half-gain", "enabled": True,
                             "effect": {"kind": "gain", "gain": 0.5, "invertPolarity": False}}]}, None],
            "sources": [dict(port(p), encoding="f64-le") for p in sources],
            "outputs": [port(p) for p in outputs]}

    def delay(name):
        return {"name": name, "format": {"sampleRateHz": 48000, "channels": ["FL", "FR"]},
                "effects": [{"name": "retained-native-tail", "enabled": True,
                             "effect": {"kind": "delay", "frames": 128, "wet": 1, "dry": 0, "feedback": 0}}]}
    if mode == "output-failure":
        result["outputDsp"][1]["effects"].append({"name": "destination-overflow", "enabled": True,
            "effect": {"kind": "gain", "gain": 16, "invertPolarity": False}})
    elif mode == "tail-loss":
        result["mixDsp"] = [None, delay("record-retained-mix"), None]
        result["outputDsp"][0] = delay("call-retained-output")
        result["outputDsp"][2] = delay("monitor-retained-output")
    elif mode != "normal":
        raise ValueError("unsupported private graph scenario")
    return result
