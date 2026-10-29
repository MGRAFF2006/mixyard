"""Real owned UDP with deterministic clock samples; no measured hardware latency."""

from contextlib import ExitStack
import socket
import struct
import threading
import unittest
from unittest.mock import patch

from mixyard_interop import Endpoint, Grant, Provenance, Session, Source, Spec
from mixyard_interop.realtime import PacketRejected, RtcpPeer, RtpProfile, RtpReceiver, RtpSender
from mixyard_interop.realtime import ReceiverReport, SenderReport


def media(codec="raw-s16-le", frames=1):
    return Spec(1, "owned synthetic", codec, 1000, ("FR", "FL"), frames,
                Provenance((Source("source", "fixture", ("synthetic",)),), ("source",)))


def grant(role, peer, spec=None, local="source", remote="destination"):
    return Session(local, (Grant(role, peer, remote, spec),), consent=True)


def udp():
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.bind(("127.0.0.1", 0))
    sock.settimeout(0.03)
    return sock


def endpoint(sock):
    return Endpoint("127.0.0.1", sock.getsockname()[1], "udp")


def unused():
    with udp() as sock:
        return endpoint(sock)


def rtcp_pair(stack, *, max_packets=256):
    a, b = unused(), unused()
    left = stack.enter_context(RtcpPeer(grant("control", b), a, b,
                               local_ssrc=7, peer_ssrc=9, cname="left", max_packets=max_packets))
    right = stack.enter_context(RtcpPeer(grant("control", a, local="destination", remote="source"), b, a,
                                local_ssrc=9, peer_ssrc=7, cname="right", max_packets=max_packets))
    return left, right


def rtp_pair(stack):
    spec, destination = media(), unused()
    sender = stack.enter_context(RtpSender(grant("live-send", destination, spec),
                                 destination, RtpProfile(spec), ssrc=7, sequence=65535,
                                 timestamp=0xFFFFFFFF))
    receiver = stack.enter_context(RtpReceiver(
        grant("live-receive", sender.local, spec, local="destination", remote="source"),
        destination, sender.local, RtpProfile(spec), ssrc=7))
    return sender, receiver


class RtcpTimingTests(unittest.TestCase):
    def test_sr_golden_compound_bytes_use_explicit_mapping_and_payload_counts(self):
        with ExitStack() as stack:
            raw = stack.enter_context(udp())
            sender, receiver = rtp_pair(stack)
            sender.send(bytes.fromhex("0080ff7f"))
            receiver.receive()
            control = stack.enter_context(RtcpPeer(grant("control", endpoint(raw)), unused(), endpoint(raw),
                                          local_ssrc=7, peer_ssrc=9, cname="fixture"))
            control.send_sender_report(sender, ntp_timestamp=0xE000000112340000, rtp_timestamp=123)
            data, _ = raw.recvfrom(1201)
            self.assertEqual(data[:28], bytes.fromhex(
                "80c8000600000007e0000001123400000000007b0000000100000004"))
            self.assertEqual(data[28:], bytes.fromhex("81ca0004000000070107") + b"fixture\0\0\0")
            self.assertEqual((sender.sent_packets, sender.sent_octets), (1, 4))

    def test_actual_udp_sr_rr_measures_local_elapsed_rtt_minus_peer_delay(self):
        with ExitStack() as stack:
            sender, receiver = rtp_pair(stack)
            sender.send(bytes.fromhex("0080ff7f"))
            received = receiver.receive()
            self.assertEqual(received.pcm, bytes.fromhex("0080ff7f"))
            left, right = rtcp_pair(stack)
            # Controlled local clock samples prove units and subtraction, not latency.
            with patch("mixyard_interop.realtime.rtcp.time.monotonic_ns", side_effect=[
                    1_000_000_000, 1_062_500_000, 1_125_000_000, 1_125_000_000, 1_250_000_000]):
                left.send_sender_report(sender, ntp_timestamp=0xE000000112340000, rtp_timestamp=123)
                sr = right.receive()
                self.assertIsInstance(sr.report, SenderReport)
                self.assertEqual((sr.report.rtp_timestamp, sr.cname, sr.round_trip_seconds), (123, "left", None))
                right.send_receiver_report(receiver)
                rr = left.receive()
            self.assertIsInstance(rr.report, ReceiverReport)
            self.assertEqual(rr.report.last_sr, 0x00011234)
            self.assertEqual(rr.report.delay_since_last_sr, 4096)
            self.assertEqual(rr.round_trip_seconds, 0.1875)
            # Repeated report without new SR has no fabricated RTT.
            with patch("mixyard_interop.realtime.rtcp.time.monotonic_ns", side_effect=[
                    1_300_000_000, 1_300_000_000, 1_400_000_000]):
                right.send_receiver_report(receiver)
                self.assertIsNone(left.receive().round_trip_seconds)

    def test_extended_counters_loss_fraction_and_jitter_in_frame_units(self):
        with udp() as source, udp() as reporter:
            spec, local = media(), unused()
            with RtpReceiver(grant("live-receive", endpoint(source), spec, local="destination", remote="source"),
                             local, endpoint(source), RtpProfile(spec), ssrc=7) as receiver:
                for seq, stamp, arrival in [(65535, 0xFFFFFFFF, 1_000_000_000), (1, 1, 1_018_000_000)]:
                    source.sendto(struct.pack("!BBHII", 0x80, 96, seq, stamp, 7) + bytes(4),
                                  (local.host, local.port))
                    with patch("mixyard_interop.realtime.rtp.time.monotonic_ns", return_value=arrival):
                        received = receiver.receive()
                self.assertEqual((received.extended_sequence, received.extended_timestamp), (65537, 4294967297))
                self.assertEqual(receiver.jitter_frames, 1.0)
                with RtcpPeer(grant("control", endpoint(reporter)), unused(), endpoint(reporter),
                              local_ssrc=9, peer_ssrc=7, cname="receiver") as control:
                    control.send_receiver_report(receiver)
                    packet, _ = reporter.recvfrom(1201)
                    self.assertEqual(packet[:4], bytes.fromhex("81c90007"))
                    fields = struct.unpack("!IIIIIII", packet[4:32])
                    self.assertEqual(fields, (9, 7, 0x55000001, 65537, 1, 0, 0))
                    control.send_receiver_report(receiver)
                    packet, _ = reporter.recvfrom(1201)
                    self.assertEqual(struct.unpack("!I", packet[12:16])[0], 1)  # No new interval loss.

    def test_malformed_or_other_source_cannot_establish_rtcp_state(self):
        with udp() as source, udp() as attacker:
            local = unused()
            with RtcpPeer(grant("control", endpoint(source)), local, endpoint(source),
                          local_ssrc=9, peer_ssrc=7, cname="receiver") as control:
                golden = bytes.fromhex("80c8000600000007e0000001123400000000007b0000000100000004"
                                       "81ca00020000000701017800")
                attacker.sendto(golden, (local.host, local.port))
                with self.assertRaisesRegex(PacketRejected, "endpoint"):
                    control.receive()
                malformed = [golden[:28], golden + b"\0\0\0\0", golden[:1] + b"\xc9" + golden[2:],
                             b"\xa0" + golden[1:], golden[:3] + b"\xff" + golden[4:],
                             golden[:7] + b"\x08" + golden[8:], golden[:-1] + b"x", bytes(1201)]
                for packet in malformed:
                    source.sendto(packet, (local.host, local.port))
                    with self.assertRaises(PacketRejected):
                        control.receive()
                    self.assertIsNone(control._last_sr)
                source.sendto(golden, (local.host, local.port))
                self.assertEqual(control.receive().cname, "x")

    def test_separate_control_grant_revocation_and_packet_budget(self):
        with ExitStack() as stack:
            sender, receiver = rtp_pair(stack)
            destination = unused()
            with self.assertRaises(PermissionError):
                RtcpPeer(sender.session, unused(), destination, local_ssrc=7, peer_ssrc=9, cname="none")
            left, right = rtcp_pair(stack, max_packets=1)
            left.send_sender_report(sender, ntp_timestamp=0xE000000112340000, rtp_timestamp=1)
            right.receive()
            with self.assertRaisesRegex(RuntimeError, "budget"):
                left.receive()
            right.session.revoke()
            with self.assertRaises(PermissionError):
                right.receive()

    def test_real_uncontrolled_clock_samples_report_rtt_without_oneway_claim(self):
        with ExitStack() as stack:
            sender, receiver = rtp_pair(stack)
            sender.send(bytes(4))
            receiver.receive()
            left, right = rtcp_pair(stack)
            left.send_sender_report(sender, ntp_timestamp=0xE000000112340000, rtp_timestamp=1)
            right.receive()
            right.send_receiver_report(receiver)
            result = left.receive()
            self.assertIsNotNone(result.round_trip_seconds)
            self.assertGreaterEqual(result.round_trip_seconds, 0)

    def test_unknown_and_impossible_rr_timing_never_fabricates_zero_rtt(self):
        with ExitStack() as stack:
            raw = stack.enter_context(udp())
            sender, _ = rtp_pair(stack)
            control = stack.enter_context(RtcpPeer(grant("control", endpoint(raw)), unused(), endpoint(raw),
                                          local_ssrc=7, peer_ssrc=9, cname="sender"))
            with patch("mixyard_interop.realtime.rtcp.time.monotonic_ns", return_value=1_000_000_000):
                control.send_sender_report(sender, ntp_timestamp=0xE000000112340000, rtp_timestamp=1)
            raw.recvfrom(1201)
            for lsr, delay in ((0x00011235, 0), (0x00011234, 65536), (0x00011234, 0)):
                report = bytes.fromhex("81c90007") + struct.pack("!IIIIIII", 9, 7, 0, 1, 0, lsr, delay)
                report += bytes.fromhex("81ca00020000000901017800")
                raw.sendto(report, (control.local.host, control.local.port))
                with patch("mixyard_interop.realtime.rtcp.time.monotonic_ns", return_value=1_250_000_000):
                    self.assertIsNone(control.receive().round_trip_seconds)

    def test_revoke_during_blocked_rtcp_receive_discards_report(self):
        with udp() as raw:
            control = RtcpPeer(grant("control", endpoint(raw)), unused(), endpoint(raw),
                               local_ssrc=9, peer_ssrc=7, cname="receiver")
            with control:
                entered, errors = threading.Event(), []
                original = control._admit
                def admit():
                    original()
                    entered.set()
                def receive():
                    try:
                        control.receive()
                    except BaseException as exc:
                        errors.append(exc)
                with patch.object(control, "_admit", side_effect=admit):
                    worker = threading.Thread(target=receive)
                    worker.start()
                    try:
                        self.assertTrue(entered.wait(0.5))
                        control.session.revoke()
                        packet = bytes.fromhex("80c8000600000007e0000001123400000000007b0000000100000004"
                                               "81ca00020000000701017800")
                        raw.sendto(packet, (control.local.host, control.local.port))
                    finally:
                        worker.join(0.5)
                self.assertFalse(worker.is_alive())
                self.assertEqual(len(errors), 1)
                self.assertIsInstance(errors[0], PermissionError)
                self.assertIsNone(control._last_sr)


if __name__ == "__main__":
    unittest.main()
