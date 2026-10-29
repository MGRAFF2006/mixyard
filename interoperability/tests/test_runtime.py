import dataclasses
import struct
import threading
import unittest

from mixyard_interop import Provenance, Source, Spec
from mixyard_interop.runtime import ClockEvidence, LeaseBinding, LeasedBlock, RuntimeBusy, RuntimeLease


def fixture(codec="raw-f64-le", groups=()):
    spec = Spec(1, "owned", codec, 48000, ("FL", "FR"), 2,
                Provenance((Source("source", "generator", groups),), ("source",)))
    binding = LeaseBinding("observed-generator", spec, 7, 3, "epoch-1", 11)
    state = [binding]
    lease = RuntimeLease(spec, observed_id=binding.observed_id, generation=7,
                         revision=3, source_epoch="epoch-1",
                         clock=ClockEvidence(11, "epoch-1", 0, 1000000),
                         owner_check=lambda: state[0], consent=True)
    return spec, binding, state, lease


def block(spec, frame=0):
    return LeasedBlock(spec, struct.pack("<4d", .125, -.25, .375, -.5),
                       7, 3, "epoch-1", frame, 11)


class RuntimeLeaseTests(unittest.TestCase):
    def test_current_observation_and_exact_block_binding(self):
        spec, binding, state, lease = fixture()
        pcm = block(spec)
        lease.validate(pcm)
        lease.validate(pcm)  # IO rechecks must not consume media.
        for field, value in (("generation", 8), ("revision", 4), ("source_epoch", "restart"),
                             ("clock_id", 12), ("spec", dataclasses.replace(spec, channels=("FR", "FL")))):
            with self.subTest(field=field), self.assertRaises(PermissionError):
                lease.validate(dataclasses.replace(pcm, **{field: value}))
        state[0] = dataclasses.replace(binding, revision=4)
        with self.assertRaises(PermissionError):
            lease.validate(pcm)
        state[0] = binding
        with self.assertRaises(PermissionError):
            lease.check()  # Restoring metadata never revives an old grant.

    def test_contiguous_delivery_revoke_and_restart(self):
        spec, _, _, lease = fixture()
        received = []
        lease.accept(block(spec), received.append)
        for frame in (0, 4):
            with self.assertRaises(PermissionError):
                lease.accept(block(spec, frame), received.append)
        lease.accept(block(spec, 2), received.append)
        lease.revoke()
        with self.assertRaises(PermissionError):
            lease.accept(block(spec, 4), received.append)
        self.assertEqual([b.base_frame for b in received], [0, 2])
        _, _, _, fresh = fixture()
        fresh.accept(block(spec), received.append)

    def test_busy_sink_does_not_queue_and_failed_sink_cannot_retry(self):
        spec, _, _, lease = fixture()
        entered, release = threading.Event(), threading.Event()
        def sink(_):
            entered.set()
            if not release.wait(1):
                raise TimeoutError("test sink deadline")
        thread = threading.Thread(target=lambda: lease.accept(block(spec), sink))
        thread.start()
        try:
            self.assertTrue(entered.wait(1))
            with self.assertRaises(RuntimeBusy):
                lease.accept(block(spec), lambda _: None)
        finally:
            release.set()
            thread.join(1)
        self.assertFalse(thread.is_alive())
        def broken(_):
            raise OSError("consumer lost after possible prefix")
        with self.assertRaises(OSError):
            lease.accept(block(spec, 2), broken)
        with self.assertRaises(PermissionError):
            lease.accept(block(spec, 2), lambda _: None)

    def test_no_partial_padding_and_raw_nonfinite_is_opaque(self):
        spec, _, _, lease = fixture()
        with self.assertRaises(ValueError):
            dataclasses.replace(block(spec), pcm=b"\0")
        payload = bytes.fromhex("010000000000f87f0000000000000080") * 2
        opaque = dataclasses.replace(block(spec), pcm=payload)
        output = []
        lease.accept(opaque, output.append)
        self.assertEqual(output[0].pcm, payload)

    def test_excluded_premix_cannot_get_lease_and_read_rechecks_owner(self):
        spec, binding, state, _ = fixture(groups=("private",))
        with self.assertRaises(PermissionError):
            RuntimeLease(spec, observed_id=binding.observed_id, generation=7, revision=3,
                         source_epoch="epoch-1", clock=ClockEvidence(11, "epoch-1", 0, 0),
                         owner_check=lambda: binding, consent=True, excluded_groups=("private",))
        _, _, _, lease = fixture()
        def stale_read():
            lease.revoke()
            return block(lease.spec)
        with self.assertRaises(PermissionError):
            lease.read(stale_read)


if __name__ == "__main__":
    unittest.main()
