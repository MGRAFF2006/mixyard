"""Finite caller-driven RFC3550 SR/RR+SDES subset, on owned loopback only.

There is no automatic report scheduler or clock synchronization. Report timestamps
are explicit external clock observations; monotonic readings measure local elapsed
time only. Unauthenticated peer fields are diagnostics, never media authority.
"""

from dataclasses import dataclass
import socket
import struct
import time

from ..core import Endpoint, Session, _identifier
from .rtp import MAX_DATAGRAM, MAX_PACKETS, PacketRejected, _endpoint, _integer, _timeout


@dataclass(frozen=True)
class RtpClockSample:
    """Caller-observed common-clock frame at the instant of SR transmission.

    ntp_timestamp is unsigned NTP 32.32. Neither this object nor SR reception
    verifies a clock source, hardware timestamp or synchronization accuracy.
    """

    source_epoch: str
    clock_id: int
    base_frame: int
    ntp_timestamp: int

    def __post_init__(self):
        _identifier(self.source_epoch)
        _integer(self.clock_id, 1, 2**64 - 1, "clock_id")
        _integer(self.base_frame, 0, 2**64 - 1, "base_frame")
        _integer(self.ntp_timestamp, 1, 2**64 - 1, "NTP timestamp")


@dataclass(frozen=True)
class SenderReport:
    ssrc: int
    ntp_timestamp: int
    rtp_timestamp: int
    packet_count: int
    octet_count: int


@dataclass(frozen=True)
class ReceiverReport:
    ssrc: int
    source_ssrc: int
    fraction_lost: int
    cumulative_lost: int
    extended_sequence: int
    jitter_frames: int
    last_sr: int
    delay_since_last_sr: int


@dataclass(frozen=True)
class ReceivedReport:
    report: SenderReport | ReceiverReport
    cname: str
    round_trip_seconds: float | None


def _packet(kind, count, body):
    return struct.pack("!BBH", 0x80 | count, kind, len(body) // 4) + body


def _sdes(ssrc, cname):
    body = struct.pack("!IBB", ssrc, 1, len(cname)) + cname + b"\0"
    return _packet(202, 1, body + bytes(-len(body) % 4))


def _parse(data, peer_ssrc, local_ssrc):
    if not 12 <= len(data) <= MAX_DATAGRAM or len(data) % 4:
        raise PacketRejected("invalid compound RTCP size")
    flags, kind, words = struct.unpack("!BBH", data[:4])
    end = (words + 1) * 4
    if kind == 200 and flags == 0x80 and end == 28 and end <= len(data):
        report = SenderReport(*struct.unpack("!IQIII", data[4:end]))
    elif kind == 201 and flags == 0x81 and end == 32 and end <= len(data):
        ssrc, source, loss, seq, jitter, lsr, dlsr = struct.unpack("!IIIIIII", data[4:end])
        lost = loss & 0xFFFFFF
        lost = lost - 0x1000000 if lost & 0x800000 else lost
        report = ReceiverReport(ssrc, source, loss >> 24, lost, seq, jitter, lsr, dlsr)
        if source != local_ssrc:
            raise PacketRejected("RR describes an unadmitted source SSRC")
    else:
        raise PacketRejected("unsupported RTCP report, count, padding or length")
    if report.ssrc != peer_ssrc:
        raise PacketRejected("unadmitted RTCP reporter SSRC")
    tail = data[end:]
    if len(tail) < 12:
        raise PacketRejected("RTCP requires compound CNAME SDES")
    flags, kind, words, ssrc, item, length = struct.unpack("!BBHIBB", tail[:10])
    expected = 10 + length
    if (flags != 0x81 or kind != 202 or (words + 1) * 4 != len(tail)
            or ssrc != peer_ssrc or item != 1 or not 1 <= length <= 128
            or len(tail) != (expected + 4) // 4 * 4
            or any(tail[expected:])):
        raise PacketRejected("invalid single-CNAME SDES or unsupported trailing packets")
    try:
        cname = tail[10:expected].decode("ascii")
        if any(not 33 <= ord(c) <= 126 for c in cname):
            raise ValueError("invalid CNAME")
    except (UnicodeError, ValueError) as exc:
        raise PacketRejected("bounded visible ASCII CNAME required") from exc
    return report, cname


class RtcpPeer:
    """One separately control-granted RTCP peer, no media or device permission.

    Single synchronous owner. At most 32 outstanding SR correlations. Replayed,
    unknown or impossible RR timing produces no RTT observation.
    """

    def __init__(self, session: Session, local: Endpoint, peer: Endpoint, *,
                 local_ssrc: int, peer_ssrc: int, cname: str,
                 timeout: float = 0.25, max_packets: int = 256):
        _endpoint(local, "udp")
        _endpoint(peer, "udp")
        _integer(local_ssrc, 0, 0xFFFFFFFF, "local_ssrc")
        _integer(peer_ssrc, 0, 0xFFFFFFFF, "peer_ssrc")
        if local_ssrc == peer_ssrc:
            raise ValueError("distinct local and peer SSRC required")
        if (type(cname) is not str or not 1 <= len(cname) <= 128
                or any(not 33 <= ord(c) <= 126 for c in cname)):
            raise ValueError("explicit bounded visible ASCII CNAME required")
        _timeout(timeout)
        _integer(max_packets, 1, MAX_PACKETS, "max_packets")
        session.require("control", peer)
        self.session, self.local, self.peer = session, local, peer
        self.local_ssrc, self.peer_ssrc = local_ssrc, peer_ssrc
        self._cname = cname.encode("ascii")
        self.operations, self.max_packets = 0, max_packets
        self._sent = {}
        self._last_sr = None
        self._previous_expected = self._previous_received = 0
        self._receiver = None
        self._socket = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            self._socket.settimeout(timeout)
            self._socket.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 16384)
            self._socket.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 16384)
            self._socket.bind((local.host, local.port))
        except BaseException:
            self._socket.close()
            raise

    def _admit(self):
        self.session.require("control", self.peer)
        if self.operations >= self.max_packets:
            raise RuntimeError("RTCP lifetime packet budget exhausted")
        self.operations += 1

    def _send(self, report):
        compound = report + _sdes(self.local_ssrc, self._cname)
        self._admit()
        sent_ns = time.monotonic_ns()
        if self._socket.sendto(compound, (self.peer.host, self.peer.port)) != len(compound):
            raise OSError("partial RTCP datagram")
        return sent_ns

    def send_sender_report(self, sender, *, ntp_timestamp, rtp_timestamp):
        """Explicit contemporaneous NTP/RTP mapping, never time.monotonic as NTP."""
        _integer(ntp_timestamp, 1, 2**64 - 1, "NTP timestamp")
        _integer(rtp_timestamp, 0, 0xFFFFFFFF, "RTP timestamp")
        if sender.ssrc != self.local_ssrc:
            raise ValueError("SR must describe the configured local RTP SSRC")
        sender.session.require("live-send", sender.peer, sender.profile.spec)
        report = _packet(200, 0, struct.pack("!IQIII", self.local_ssrc, ntp_timestamp,
                         rtp_timestamp, sender.sent_packets & 0xFFFFFFFF,
                         sender.sent_octets & 0xFFFFFFFF))
        lsr = (ntp_timestamp >> 16) & 0xFFFFFFFF
        # Ambiguous repeated compact timestamps cannot correlate an RTT safely.
        if lsr and lsr in self._sent:
            raise ValueError("outstanding compact NTP timestamp must be unique")
        sent_ns = self._send(report)
        if lsr:
            if len(self._sent) >= 32:
                del self._sent[next(iter(self._sent))]
            self._sent[lsr] = sent_ns

    def send_receiver_report(self, receiver):
        if receiver.ssrc != self.peer_ssrc or receiver.extended_sequence is None:
            raise ValueError("RR requires admitted RTP samples from the configured peer SSRC")
        if self._receiver is not None and self._receiver is not receiver:
            raise ValueError("fresh RTCP peer required for a new RTP receive epoch")
        receiver.session.require("live-receive", receiver.peer, receiver.profile.spec)
        expected, received = receiver.received + receiver.missing, receiver.received
        interval = expected - self._previous_expected
        lost_interval = interval - (received - self._previous_received)
        fraction = min(255, (lost_interval << 8) // interval) if interval > 0 and lost_interval > 0 else 0
        lsr = dlsr = 0
        if self._last_sr is not None:
            lsr, arrival_ns = self._last_sr
            elapsed = time.monotonic_ns() - arrival_ns
            if not 0 <= elapsed < 65536 * 1_000_000_000:
                raise ValueError("SR observation expired or local monotonic clock regressed")
            dlsr = elapsed * 65536 // 1_000_000_000 if lsr else 0
        loss = (fraction << 24) | min(0x7FFFFF, receiver.missing)
        report = _packet(201, 1, struct.pack("!IIIIIII", self.local_ssrc, receiver.ssrc,
                         loss, receiver.extended_sequence & 0xFFFFFFFF,
                         min(0xFFFFFFFF, int(receiver.jitter_frames)), lsr, dlsr))
        self._send(report)
        self._receiver = receiver
        self._previous_expected, self._previous_received = expected, received

    def receive(self):
        self._admit()
        data, address = self._socket.recvfrom(MAX_DATAGRAM + 1)
        arrival_ns = time.monotonic_ns()
        self.session.require("control", self.peer)
        if address != (self.peer.host, self.peer.port):
            raise PacketRejected("unadmitted RTCP endpoint")
        report, cname = _parse(data, self.peer_ssrc, self.local_ssrc)
        self.session.require("control", self.peer)
        rtt = None
        if isinstance(report, SenderReport):
            self._last_sr = ((report.ntp_timestamp >> 16) & 0xFFFFFFFF, arrival_ns)
        elif report.last_sr in self._sent:
            elapsed_ns = arrival_ns - self._sent.pop(report.last_sr)
            delay_ns = report.delay_since_last_sr * 1_000_000_000 // 65536
            if 0 <= delay_ns <= elapsed_ns < 65536 * 1_000_000_000:
                rtt = (elapsed_ns - delay_ns) / 1_000_000_000
        return ReceivedReport(report, cname, rtt)

    def close(self):
        self._socket.close()
        self._sent.clear()
        self._last_sr = None

    def __enter__(self):
        return self

    def __exit__(self, *_exc):
        self.close()
