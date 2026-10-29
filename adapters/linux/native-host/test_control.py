"""Control-plane tests; these never import a native library or start PipeWire."""
import copy
import unittest
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
