"""Actual RuntimeLease -> owned UDP -> RuntimeLease, with synthetic callbacks.

This leaf depends on the runtime concern. Standalone main-based checkouts skip
explicitly until integration; the runtime parent executes this against its real
module by extending mixyard_interop.__path__ in the test launcher, without copies.
"""

from contextlib import ExitStack
from dataclasses import replace
import importlib.util
import socket
import struct
import threading
import unittest
from unittest.mock import patch

from mixyard_interop import Endpoint, Grant, Provenance, Session, Source, Spec
from mixyard_interop.realtime import PacketRejected, RtpClockSample, RtpProfile, RtpReceiver, RtpSender

HAS_RUNTIME = importlib.util.find_spec("mixyard_interop.runtime") is not None
if HAS_RUNTIME:
    from mixyard_interop.runtime import ClockEvidence, LeaseBinding, LeasedBlock, RuntimeLease
    from mixyard_interop.realtime.leased import LeasedRtpReceiver, LeasedRtpSender


def owned_media(codec="raw-s16-le"):
    return Spec(1, "owned fixture", codec, 48000, ("FR", "FL"), 2,
                Provenance((Source("source", "fixture", ("synthetic",)),), ("source",)))


def endpoint(sock):
    return Endpoint("127.0.0.1", sock.getsockname()[1], "udp")


def unused():
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return endpoint(sock)


def lease(spec, epoch="epoch-1", clock=11, frame=8):
    evidence = ClockEvidence(clock, epoch, frame, 1_000_000)
    binding = LeaseBinding("observed-fixture", spec, 7, 3, epoch, clock, evidence)
    current = [binding]
    result = RuntimeLease(spec, observed_id=binding.observed_id, generation=7, revision=3,
                          source_epoch=epoch, clock=evidence,
                          owner_check=lambda: current[0], consent=True)
    return result, current


def binding(epoch="epoch-1", clock=11, frame=8):
    return dict(generation=7, revision=3, source_epoch=epoch, base_frame=frame, clock_id=clock)


def block(spec, frame=8, *, epoch="epoch-1", clock=11, pcm=None):
    if pcm is None:
        pcm = bytes.fromhex("0080ff7f0100ffff") if spec.codec == "raw-s16-le" else bytes.fromhex(
            "000080ffff7f010000ffffff")
    return LeasedBlock(spec, pcm, 7, 3, epoch, frame, clock)


def pair(stack, spec, *, epoch="epoch-1", clock=11, frame=8, ssrc=7):
    destination = unused()
    outgoing = Session("source", (Grant("live-send", destination, "destination", spec),), consent=True)
    sender = stack.enter_context(RtpSender(outgoing, destination, RtpProfile(spec), ssrc=ssrc,
                                          sequence=65535, timestamp=0xFFFFFFFF, timeout=0.03))
    incoming = Session("destination", (Grant("live-receive", sender.local, "source", spec),), consent=True)
    receiver = stack.enter_context(RtpReceiver(incoming, destination, sender.local, RtpProfile(spec),
                                              ssrc=ssrc, timeout=0.1))
    send_lease, send_owner = lease(spec, epoch, clock, frame)
    receive_lease, receive_owner = lease(spec, epoch, clock, frame)
    return sender, receiver, send_lease, send_owner, receive_lease, receive_owner


@unittest.skipUnless(HAS_RUNTIME, "requires parent runtime lease concern (actual RuntimeLease, no stub)")
class LeasedRtpTests(unittest.TestCase):
    def test_actual_leases_send_receive_exact_bytes_across_rollover_and_distinct_f64_processing(self):
        for codec in ("raw-s16-le", "raw-s24-le"):
            with self.subTest(codec=codec), ExitStack() as stack:
                spec = owned_media(codec)
                width = 2 if codec == "raw-s16-le" else 3
                sender, receiver, tx, _, rx, _ = pair(stack, spec)
                packets = iter([block(spec), block(spec, 10)])
                raw, processed = [], []
                def host_sink(received):
                    raw.append(received)
                    # Deliberate destination-local host processing; never raw identity evidence.
                    samples = [int.from_bytes(received.pcm[i:i+width], "little", signed=True)
                               / (1 << (width * 8 - 1)) * 0.5
                               for i in range(0, len(received.pcm), width)]
                    processed.append(struct.pack("<4d", *samples))
                output = LeasedRtpSender(sender, tx, lambda: next(packets), **binding())
                input_ = LeasedRtpReceiver(receiver, rx, host_sink, timestamp=0xFFFFFFFF, **binding())
                self.assertEqual(output.send(), 65535)
                first = input_.receive()
                self.assertEqual(first, block(spec))
                self.assertEqual(output.send(), 0)
                second = input_.receive()
                self.assertEqual((second.base_frame, receiver.extended_sequence), (10, 65536))
                self.assertEqual([b.pcm for b in raw], [block(spec).pcm] * 2)
                self.assertEqual(struct.unpack("<4d", processed[0])[0], -0.5)
                self.assertEqual(struct.unpack("<4d", processed[0])[1], 0.5 - 0.5 / (1 << (width * 8 - 1)))
                self.assertEqual(len(processed[0]), 32)
                self.assertNotEqual(processed[0], first.pcm)

    def test_source_change_and_block_mismatch_reject_before_send(self):
        for field, value in (("generation", 8), ("revision", 4), ("source_epoch", "epoch-old"),
                             ("clock_id", 12), ("base_frame", 10)):
            with self.subTest(field=field), ExitStack() as stack:
                spec = owned_media()
                sender, receiver, tx, _, _, _ = pair(stack, spec)
                bad = replace(block(spec), **{field: value})
                output = LeasedRtpSender(sender, tx, lambda: bad, **binding())
                with self.assertRaises(PermissionError):
                    output.send()
                self.assertEqual(sender.sent_packets, 0)
                with self.assertRaises(socket.timeout):
                    receiver.receive()
        with ExitStack() as stack:
            spec = owned_media()
            sender, _, tx, state, _, _ = pair(stack, spec)
            output = LeasedRtpSender(sender, tx, lambda: block(spec), **binding())
            state[0] = replace(state[0], generation=8)
            with self.assertRaises(PermissionError):
                output.send()
            self.assertEqual(sender.sent_packets, 0)

    def test_partial_frame_packet_and_spec_mismatch_cannot_silently_convert(self):
        with ExitStack() as stack:
            spec = owned_media()
            sender, _, tx, _, _, _ = pair(stack, spec)
            output = LeasedRtpSender(sender, tx, lambda: block(spec, pcm=bytes(4)), **binding())
            with self.assertRaisesRegex(ValueError, "full packet"):
                output.send()
            with self.assertRaises(PermissionError):
                LeasedRtpSender(sender, tx, lambda: block(spec), **binding(clock=12))
            with self.assertRaises(PermissionError):
                LeasedRtpSender(sender, tx, lambda: block(spec), **binding(frame=10))
            self.assertEqual(sender.sent_packets, 0)
        with self.assertRaises(ValueError):
            RtpProfile(owned_media("raw-f64-le"))

    def test_revoked_source_during_callback_and_sink_during_udp_wait_release_nothing(self):
        with ExitStack() as stack:
            spec = owned_media()
            sender, receiver, tx, _, rx, _ = pair(stack, spec)
            def stale():
                tx.revoke()
                return block(spec)
            output = LeasedRtpSender(sender, tx, stale, **binding())
            with self.assertRaises(PermissionError):
                output.send()
            self.assertEqual(sender.sent_packets, 0)
            accepted = []
            input_ = LeasedRtpReceiver(receiver, rx, accepted.append, timestamp=0xFFFFFFFF, **binding())
            entered = threading.Event()
            original = receiver._admit
            errors = []
            def admit():
                original()
                entered.set()
            def receive():
                try:
                    input_.receive()
                except BaseException as exc:
                    errors.append(exc)
            with patch.object(receiver, "_admit", side_effect=admit):
                worker = threading.Thread(target=receive)
                worker.start()
                try:
                    self.assertTrue(entered.wait(0.5))
                    rx.revoke()
                    sender.send(block(spec).pcm)
                finally:
                    worker.join(0.5)
            self.assertFalse(worker.is_alive())
            self.assertEqual(accepted, [])
            self.assertEqual(len(errors), 1)
            self.assertIsInstance(errors[0], PermissionError)

    def test_receive_loss_and_failed_sink_require_explicit_recovery(self):
        with ExitStack() as stack:
            spec = owned_media()
            sender, receiver, _, _, rx, _ = pair(stack, spec)
            accepted = []
            input_ = LeasedRtpReceiver(receiver, rx, accepted.append, timestamp=0xFFFFFFFF, **binding())
            sender.send(block(spec).pcm)
            input_.receive()
            sender.sequence = 1
            sender.timestamp = 3
            sender.send(block(spec).pcm)
            with self.assertRaisesRegex(PermissionError, "discontinuity"):
                input_.receive()
            self.assertEqual([b.base_frame for b in accepted], [8])
        with ExitStack() as stack:
            spec = owned_media()
            sender, receiver, _, _, rx, _ = pair(stack, spec)
            def failed(_):
                raise OSError("possibly consumed prefix")
            input_ = LeasedRtpReceiver(receiver, rx, failed, timestamp=0xFFFFFFFF, **binding())
            sender.send(block(spec).pcm)
            with self.assertRaises(OSError):
                input_.receive()
            with self.assertRaises(PermissionError):
                rx.check()

    def test_restart_needs_new_lease_and_ssrc_and_stale_wire_cannot_enter_new_epoch(self):
        with ExitStack() as stack:
            spec = owned_media()
            sender, receiver, tx, _, rx, _ = pair(stack, spec)
            output = LeasedRtpSender(sender, tx, lambda: block(spec), **binding())
            output.send()
            receiver.receive()
            with self.assertRaises(ValueError):
                LeasedRtpSender(sender, tx, lambda: block(spec), **binding())
            tx.revoke()
            rx.revoke()
        with ExitStack() as stack:
            sender, receiver, tx, _, rx, _ = pair(stack, spec, epoch="epoch-2", ssrc=8)
            accepted = []
            output = LeasedRtpSender(sender, tx, lambda: block(spec, epoch="epoch-2"), **binding(epoch="epoch-2"))
            input_ = LeasedRtpReceiver(receiver, rx, accepted.append, timestamp=0xFFFFFFFF,
                                      **binding(epoch="epoch-2"))
            stale = struct.pack("!BBHII", 0x80, 96, 65535, 0xFFFFFFFF, 7) + bytes(8)
            sender._socket.sendto(stale, (receiver.local.host, receiver.local.port))
            with self.assertRaises(PacketRejected):
                input_.receive()
            output.send()
            self.assertEqual(input_.receive().source_epoch, "epoch-2")
            self.assertEqual(len(accepted), 1)

    def test_common_clock_report_requires_matching_explicit_external_observation(self):
        from mixyard_interop.realtime import RtcpPeer
        with ExitStack() as stack:
            spec = owned_media()
            sender, _, tx, _, _, _ = pair(stack, spec)
            output = LeasedRtpSender(sender, tx, lambda: block(spec), **binding())
            raw = stack.enter_context(socket.socket(socket.AF_INET, socket.SOCK_DGRAM))
            raw.bind(("127.0.0.1", 0))
            raw.settimeout(0.1)
            ep = endpoint(raw)
            grant = Session("source", (Grant("control", ep, "destination"),), consent=True)
            control = stack.enter_context(RtcpPeer(grant, unused(), ep, local_ssrc=7, peer_ssrc=9, cname="fixture"))
            for sample in (RtpClockSample("stale", 11, 8, 0xE000000112340000),
                           RtpClockSample("epoch-1", 12, 8, 0xE000000112340000)):
                with self.assertRaises(PermissionError):
                    output.send_report(control, sample)
            output.send_report(control, RtpClockSample("epoch-1", 11, 10, 0xE000000112340000))
            packet, _ = raw.recvfrom(1201)
            self.assertEqual(struct.unpack("!I", packet[16:20])[0], 1)

    def test_receive_owner_restart_clock_change_and_first_timestamp_reject_before_sink(self):
        for field, value in (("generation", 8), ("revision", 4), ("source_epoch", "epoch-2"),
                             ("clock_id", 12), ("base_time_ns", 2_000_000)):
            with self.subTest(field=field), ExitStack() as stack:
                spec = owned_media()
                sender, receiver, _, _, rx, owner = pair(stack, spec)
                accepted = []
                input_ = LeasedRtpReceiver(receiver, rx, accepted.append, timestamp=0xFFFFFFFF, **binding())
                sender.send(block(spec).pcm)
                if field in ("source_epoch", "clock_id"):
                    owner[0] = replace(owner[0], **{field: value},
                                       clock_evidence=replace(rx.clock, **{field: value}))
                elif field == "base_time_ns":
                    owner[0] = replace(owner[0], clock_evidence=replace(rx.clock, base_time_ns=value))
                else:
                    owner[0] = replace(owner[0], **{field: value})
                with self.assertRaises(PermissionError):
                    input_.receive()
                self.assertEqual(accepted, [])
        with ExitStack() as stack:
            spec = owned_media()
            sender, receiver, _, _, rx, _ = pair(stack, spec)
            accepted = []
            input_ = LeasedRtpReceiver(receiver, rx, accepted.append, timestamp=0xFFFFFFFF, **binding())
            sender.timestamp = 0
            sender.send(block(spec).pcm)
            with self.assertRaisesRegex(PermissionError, "first RTP timestamp"):
                input_.receive()
            self.assertEqual(accepted, [])


if __name__ == "__main__":
    unittest.main()
