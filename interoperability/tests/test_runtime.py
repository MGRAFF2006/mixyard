import dataclasses
import struct
import threading
import unittest

from mixyard_interop import Provenance, Source, Spec
from mixyard_interop.runtime import (ClockEvidence, LeaseBinding, LeasedBlock, RuntimeBusy,
                                    RuntimeLease, RuntimeSourceLost)


def fixture(codec="raw-f64-le", groups=()):
    spec = Spec(1, "owned", codec, 48000, ("FL", "FR"), 2,
                Provenance((Source("source", "generator", groups),), ("source",)))
    clock = ClockEvidence(11, "epoch-1", 0, 1000000)
    binding = LeaseBinding("observed-generator", spec, 7, 3, "epoch-1", 11, clock)
    state = [binding]
    lease = RuntimeLease(spec, observed_id=binding.observed_id, generation=7,
                         revision=3, source_epoch="epoch-1",
                         clock=clock,
                         owner_check=lambda: state[0], consent=True)
    return spec, binding, state, lease


def block(spec, frame=0):
    return LeasedBlock(spec, struct.pack("<4d", .125, -.25, .375, -.5),
                       7, 3, "epoch-1", frame, 11)


class RuntimeLeaseTests(unittest.TestCase):
    def test_producer_graph_lock_is_released_before_owner_and_consumer_callbacks(self):
        from mixyard_interop.runtime import _PRODUCER_GRAPH_LOCK, _ProducerReceipt
        spec, binding, _, actual = fixture()
        def owner():
            self.assertFalse(_PRODUCER_GRAPH_LOCK.locked())
            return binding
        actual._owner_check = owner
        guarded = dataclasses.replace(block(spec), _producer=_ProducerReceipt(threading.Event()))
        def consume(value):
            self.assertFalse(_PRODUCER_GRAPH_LOCK.locked())
        actual.accept(guarded, consume)

    def test_production_owner_callback_revoke_and_source_loss_admit_no_consumer(self):
        spec, binding, _, _ = fixture()
        holder, consumed = {}, []
        def current_owner():
            if holder.get("armed"):
                holder["lease"].revoke()
            return binding
        actual = RuntimeLease(spec, observed_id=binding.observed_id, generation=7, revision=3,
                              source_epoch=binding.source_epoch, clock=binding.clock_evidence,
                              owner_check=current_owner, consent=True)
        holder.update(lease=actual, armed=True)
        with self.assertRaises(PermissionError):
            actual.accept(block(spec), consumed.append)
        self.assertEqual(consumed, [])
        _, _, _, lost = fixture()
        lost.source_lost()
        with self.assertRaises(RuntimeSourceLost):
            lost.read(lambda: block(spec))
        with self.assertRaises(RuntimeSourceLost):
            lost.accept(block(spec), consumed.append)
        self.assertEqual(consumed, [])


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

    def test_reentrant_owner_revoke_or_observation_never_invokes_consumer(self):
        for action in ("revoke", "check", "accept"):
            with self.subTest(action=action):
                spec, binding, state, lease = fixture()
                consumed = []
                def owner_check():
                    if action == "revoke":
                        lease.revoke()
                    else:
                        try:
                            if action == "check":
                                lease.check()
                            else:
                                lease.accept(block(spec), consumed.append)
                        except RuntimeBusy:
                            pass  # Even a callback swallowing reentry cannot rearm.
                    return binding
                lease._owner_check = owner_check
                with self.assertRaises((PermissionError, RuntimeBusy)):
                    lease.accept(block(spec), consumed.append)
                self.assertEqual(consumed, [])

    def test_owner_attests_complete_clock_mapping_not_only_numeric_id(self):
        for field, value in (("base_frame", 1), ("base_time_ns", 1000001),
                             ("qualification", "qualified-owner-common-clock")):
            with self.subTest(field=field):
                spec, binding, state, lease = fixture()
                state[0] = dataclasses.replace(binding, clock_evidence=dataclasses.replace(
                    binding.clock_evidence, **{field: value}))
                with self.assertRaises(PermissionError):
                    lease.accept(block(spec), lambda _: self.fail("stale clock reached sink"))

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



class ProducerReceiptTests(unittest.TestCase):
    def test_late_growth_is_live_without_upstream_self_poll(self):
        from mixyard_interop.runtime import _ProducerReceipt
        upstream, downstream = (_ProducerReceipt(threading.Event()) for _ in range(2))
        downstream._include((upstream,))
        stopped = threading.Event()
        upstream._include((_ProducerReceipt(stopped),))
        stopped.set()
        with self.assertRaises(RuntimeSourceLost):
            downstream.check()

    def test_cycle_rejection_and_empty_growth_preserve_monotonic_scoped_restrictions(self):
        from mixyard_interop.runtime import _ProducerReceipt
        upstream, affected, dependent, unrelated = (_ProducerReceipt(threading.Event()) for _ in range(4))
        affected._include((upstream,))
        affected._include(())
        dependent._include((affected,))
        with self.assertRaisesRegex(RuntimeSourceLost, "cycle"):
            affected._include((dependent,))
        with self.assertRaises(RuntimeSourceLost):
            affected.check()
        with self.assertRaises(RuntimeSourceLost):
            dependent.check()
        upstream.check()
        unrelated.check()

    def test_deep_live_graph_has_exact_node_bound_without_recursive_traversal(self):
        from mixyard_interop.runtime import _ProducerReceipt
        stopped = threading.Event()
        root = _ProducerReceipt(stopped)
        for _ in range(1023):
            next_node = _ProducerReceipt(threading.Event())
            next_node._include((root,))
            root = next_node
        root.check()  # 1024 nodes and 1023 edges, exceeding Python recursion depth.
        excess = _ProducerReceipt(threading.Event())
        with self.assertRaisesRegex(RuntimeSourceLost, "node budget"):
            excess._include((root,))
        root.check()  # The rejected dependent cannot revoke its healthy upstream.
        stopped.set()
        with self.assertRaises(RuntimeSourceLost):
            root.check()

    def test_shared_dag_counts_edges_and_rejects_later_overflow_without_truncation(self):
        from mixyard_interop.runtime import _ProducerReceipt
        leaves = [_ProducerReceipt(threading.Event()) for _ in range(64)]
        branches = [_ProducerReceipt(threading.Event()) for _ in range(63)]
        for branch in branches:
            branch._include(leaves)
        hub = _ProducerReceipt(threading.Event())
        hub._include(branches)  # 4095 edges, shared leaves are valid DAG ancestors.
        boundary = _ProducerReceipt(threading.Event())
        boundary._include((hub,))  # Exactly 4096 edges.
        boundary.check()
        excess = _ProducerReceipt(threading.Event())
        with self.assertRaisesRegex(RuntimeSourceLost, "edge budget"):
            excess._include((boundary,))
        with self.assertRaises(RuntimeSourceLost):
            excess.check()
        boundary.check()


if __name__ == "__main__":
    unittest.main()
