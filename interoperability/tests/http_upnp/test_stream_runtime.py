"""Actual lease integration; runs when the parent runtime prerequisite is present."""
import struct
import threading
import unittest
from dataclasses import replace

from mixyard_interop.http_upnp import ProtocolError, Resource, StreamReceiver, StreamSource
if __package__:
    from .test_stream import endpoint, metadata, peer, response_head, sessions, spec, wire_record
else:
    from test_stream import endpoint, metadata, peer, response_head, sessions, spec, wire_record

try:
    from mixyard_interop.runtime import ClockEvidence, LeaseBinding, LeasedBlock, RuntimeLease
except ModuleNotFoundError as exc:
    if exc.name != "mixyard_interop.runtime":
        raise
    RuntimeLease = None


@unittest.skipIf(RuntimeLease is None, "parent runtime lease prerequisite absent")
class RuntimeStreamTests(unittest.TestCase):
    def test_session_revoked_during_actual_runtime_accept_never_invokes_consumer(self):
        fmt = spec()
        clock = ClockEvidence(7, "owned-epoch", 0, 1000000000)
        binding = LeaseBinding("owned-fixture", fmt, 3, 5, "owned-epoch", 7, clock)
        armed, calls, consumed = [False], [], []
        with peer(response_head(fmt) + wire_record(fmt) + b"0\r\n\r\n") as ep:
            incoming = sessions(fmt, ep)[1]
            def owner():
                if armed[0]:
                    calls.append(1)
                    # One transport check, then the actual lease.accept owner
                    # callback immediately before consumer admission.
                    if len(calls) >= 2:
                        incoming.revoke()
                return binding
            lease = self.lease(fmt, owner=owner)
            def factory(**value):
                armed[0] = True
                return LeasedBlock(**value)
            with self.assertRaises(PermissionError):
                StreamReceiver(incoming).receive(Resource(ep, "/stream", fmt), consumed.append,
                    accept_block=lease.accept, check_lease=lease.check, block_factory=factory)
        self.assertGreaterEqual(len(calls), 2)
        self.assertEqual(consumed, [])

    def test_owner_revoking_source_grant_prevents_listener_creation(self):
        fmt, ep = spec(), endpoint()
        outgoing = sessions(fmt, ep)[0]
        binding = LeaseBinding("owned-fixture", fmt, 3, 5, "owned-epoch", 7,
                               ClockEvidence(7, "owned-epoch", 0, 1000000000))
        armed, reads = [False], []
        def owner():
            if armed[0]:
                outgoing.revoke()
            return binding
        lease = self.lease(fmt, owner=owner)
        armed[0] = True
        with self.assertRaises(PermissionError):
            StreamSource(outgoing, fmt, lambda: reads.append(1), check_lease=lease.check,
                         validate_block=lease.validate, endpoint=ep)
        self.assertEqual(reads, [])

    def test_receive_reentry_rejects_before_opening_another_connection(self):
        fmt = spec()
        with peer(response_head(fmt) + wire_record(fmt) + b"0\r\n\r\n") as ep:
            lease, consumed = self.lease(fmt), []
            receiver = StreamReceiver(sessions(fmt, ep)[1])
            def sink(block):
                consumed.append(block)
                with self.assertRaisesRegex(RuntimeError, "worker busy"):
                    receiver.receive(Resource(ep, "/stream", fmt), sink, accept_block=lease.accept,
                                     check_lease=lease.check, block_factory=LeasedBlock)
            result = receiver.receive(Resource(ep, "/stream", fmt), sink, accept_block=lease.accept,
                                      check_lease=lease.check, block_factory=LeasedBlock)
            self.assertEqual((result.blocks, len(consumed)), (1, 1))

    def lease(self, fmt, *, owner=None, lifetime=60):
        clock = ClockEvidence(7, "owned-epoch", 0, 1000000000)
        binding = LeaseBinding("owned-fixture", fmt, 3, 5, "owned-epoch", 7, clock)
        return RuntimeLease(fmt, observed_id=binding.observed_id, generation=binding.generation,
                            revision=binding.revision, source_epoch=binding.source_epoch,
                            clock=clock,
                            owner_check=owner if owner is not None else lambda: binding,
                            consent=True, lifetime_seconds=lifetime)

    def test_real_source_and_destination_leases_raw_bits_and_timeline_roundtrip(self):
        # Opaque HTTP raw preserves nonfinite and low-bit patterns. This is not
        # the processed host's finite Float64 output, nor a host DSP proof.
        for codec, samples in (
            ("raw-s16-le", bytes.fromhex("0080ff7f0100")),
            ("raw-s24-le", bytes.fromhex("000080ffff7f010000")),
            ("raw-s32-le", bytes.fromhex("00000080ffffff7f01000000")),
            ("raw-f32-le", struct.pack("<III", 0x7fc01234, 0x80000000, 1)),
            ("raw-f64-le", struct.pack("<QQQ", 0x7ff8000000001234, 0x8000000000000000, 1)),
        ):
            with self.subTest(codec=codec):
                fmt, ep = spec(codec, rate=96000, frames=3, channels=("FC",)), endpoint()
                send, receive = sessions(fmt, ep)
                source_lease, destination_lease = self.lease(fmt), self.lease(fmt)
                original = [LeasedBlock(fmt, samples, 3, 5, "owned-epoch", base, 7)
                            for base in (0, 3, 6)]
                queue = iter(original)
                delivered = []
                with StreamSource(send, fmt, lambda: source_lease.read(lambda: next(queue, None)),
                                  check_lease=source_lease.check, validate_block=source_lease.validate,
                                  endpoint=ep) as source:
                    result = StreamReceiver(receive).receive(
                        source.resource, delivered.append, accept_block=destination_lease.accept,
                        check_lease=destination_lease.check, block_factory=LeasedBlock)
                self.assertEqual(result.blocks, 3)
                self.assertEqual(delivered, original)
                self.assertEqual(b"".join(b.pcm for b in delivered), samples * 3)

    def test_runtime_boundary_rejects_stale_metadata_gaps_and_repeats(self):
        fmt = spec()
        cases = {"generation": metadata(generation=4), "revision": metadata(revision=6),
                 "epoch": metadata(source_epoch="new-epoch"), "clock": metadata(clock_id=8),
                 "first_frame": metadata(base_frame=20)}
        for name, meta in cases.items():
            with self.subTest(case=name), peer(response_head(fmt) + wire_record(fmt, meta) + b"0\r\n\r\n") as ep:
                lease, delivered = self.lease(fmt), []
                with self.assertRaises(PermissionError):
                    StreamReceiver(sessions(fmt, ep)[1]).receive(
                        Resource(ep, "/stream", fmt), delivered.append, accept_block=lease.accept,
                        check_lease=lease.check, block_factory=LeasedBlock)
                self.assertEqual(delivered, [])
        for next_frame in (0, 40):
            response = response_head(fmt) + wire_record(fmt) + wire_record(fmt, metadata(base_frame=next_frame))
            with self.subTest(next_frame=next_frame), peer(response) as ep:
                lease, delivered = self.lease(fmt), []
                with self.assertRaises(PermissionError):
                    StreamReceiver(sessions(fmt, ep)[1]).receive(
                        Resource(ep, "/stream", fmt), delivered.append, accept_block=lease.accept,
                        check_lease=lease.check, block_factory=LeasedBlock)
                self.assertEqual(len(delivered), 1)

    def test_source_owner_revision_change_while_pacing_stops_before_second_source_read(self):
        fmt, ep = spec(rate=20), endpoint()
        binding = LeaseBinding("owned-fixture", fmt, 3, 5, "owned-epoch", 7,
                               ClockEvidence(7, "owned-epoch", 0, 1000000000))
        current = [binding]
        source_lease = self.lease(fmt, owner=lambda: current[0])
        destination_lease = self.lease(fmt)
        calls, delivered = [], []

        def source_read():
            calls.append(1)
            return LeasedBlock(fmt, bytes(fmt.frame_bytes * 20), 3, 5, "owned-epoch", 0, 7)

        def consume(value):
            delivered.append(value)
            current[0] = replace(binding, revision=6)

        with StreamSource(sessions(fmt, ep)[0], fmt, lambda: source_lease.read(source_read),
                          check_lease=source_lease.check, validate_block=source_lease.validate,
                          endpoint=ep) as source:
            with self.assertRaises((ProtocolError, OSError)):
                StreamReceiver(sessions(fmt, ep)[1]).receive(
                    source.resource, consume, accept_block=destination_lease.accept,
                    check_lease=destination_lease.check, block_factory=LeasedBlock)
        self.assertEqual(len(delivered), 1)
        self.assertEqual(calls, [1])

    def test_destination_owner_change_during_partial_record_releases_no_pcm(self):
        fmt = spec()
        sent, release = threading.Event(), threading.Event()
        binding = LeaseBinding("owned-fixture", fmt, 3, 5, "owned-epoch", 7,
                               ClockEvidence(7, "owned-epoch", 0, 1000000000))
        current = [binding]
        lease = self.lease(fmt, owner=lambda: current[0])

        def stall(conn):
            sent.set()
            release.wait(1)

        with peer(response_head(fmt) + b"100\r\nx", after_send=stall) as ep:
            delivered, errors = [], []

            def run():
                try:
                    StreamReceiver(sessions(fmt, ep)[1]).receive(
                        Resource(ep, "/stream", fmt), delivered.append, accept_block=lease.accept,
                        check_lease=lease.check, block_factory=LeasedBlock)
                except Exception as exc:
                    errors.append(exc)

            worker = threading.Thread(target=run)
            worker.start()
            try:
                self.assertTrue(sent.wait(1))
                current[0] = replace(
                    binding, source_epoch="replacement-source",
                    clock_evidence=replace(binding.clock_evidence, source_epoch="replacement-source"))
                worker.join(0.5)
                self.assertFalse(worker.is_alive())
                self.assertEqual(delivered, [])
                self.assertEqual(len(errors), 1)
                self.assertIsInstance(errors[0], PermissionError)
            finally:
                release.set()
                worker.join(2)

    def test_destination_callback_failure_is_terminal_and_does_not_retry(self):
        fmt = spec()
        response = response_head(fmt) + wire_record(fmt) + wire_record(fmt, metadata(base_frame=20))
        lease, consumed = self.lease(fmt), []

        def fail(block):
            consumed.append(block)
            raise RuntimeError("synthetic consumer consumed a prefix then failed")

        with peer(response) as ep:
            with self.assertRaises(RuntimeError):
                StreamReceiver(sessions(fmt, ep)[1]).receive(
                    Resource(ep, "/stream", fmt), fail, accept_block=lease.accept,
                    check_lease=lease.check, block_factory=LeasedBlock)
        self.assertEqual(len(consumed), 1)
        with self.assertRaises(PermissionError):
            lease.check()


if __name__ == "__main__":
    unittest.main()
