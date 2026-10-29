"""Bounded RTP v2 L16/L24 datagrams; no device, discovery, RTCP or SRTP IO."""

from dataclasses import dataclass
import secrets
import socket
import struct

from ..core import Endpoint, Session, Spec

MAX_DATAGRAM = 1200
MAX_PACKETS = 65536
MAX_GAP = 128


class PacketRejected(ValueError):
    """Malformed or out-of-profile packet; no samples have been delivered."""


def _integer(value: int, lower: int, upper: int, name: str) -> None:
    if type(value) is not int or not lower <= value <= upper:
        raise ValueError(f"{name} must be an integer in {lower}..{upper}")


def _endpoint(endpoint: Endpoint, scheme: str) -> None:
    if not isinstance(endpoint, Endpoint) or endpoint.host != "127.0.0.1":
        raise ValueError("explicit literal IPv4 loopback Endpoint required")
    _integer(endpoint.port, 1, 65535, "port")
    if endpoint.scheme != scheme:
        raise ValueError(f"{scheme} endpoint required")


def _timeout(value: float) -> None:
    if type(value) not in (float, int) or not 0 < value <= 1:
        raise ValueError("timeout must be greater than zero and at most one second")


@dataclass(frozen=True)
class RtpProfile:
    """Out-of-band exact format agreement, including ordered channel identity."""

    spec: Spec
    payload_type: int = 96

    def __post_init__(self) -> None:
        if not isinstance(self.spec, Spec):
            raise ValueError("canonical Spec required")
        if self.spec.codec not in ("raw-s16-le", "raw-s24-le"):
            raise ValueError("this profile supports only raw-s16-le and raw-s24-le")
        _integer(self.payload_type, 96, 127, "dynamic payload type")
        if self.payload_bytes + 12 > MAX_DATAGRAM:
            raise ValueError("exact format exceeds 1200-byte RTP datagram budget")

    @property
    def sample_bytes(self) -> int:
        return 2 if self.spec.codec == "raw-s16-le" else 3

    @property
    def payload_bytes(self) -> int:
        return self.spec.frames_per_packet * self.spec.frame_bytes

    @property
    def encoding(self) -> str:
        return "L16" if self.sample_bytes == 2 else "L24"

    def convert_endian(self, payload: bytes) -> bytes:
        """Explicit reversible destination-local LE <-> network BE conversion."""
        if type(payload) is not bytes or len(payload) != self.payload_bytes:
            raise ValueError("one exact PCM packet required; no padding or truncation")
        width = self.sample_bytes
        return b"".join(payload[i:i + width][::-1] for i in range(0, len(payload), width))

    def sdp(self, destination: Endpoint) -> bytes:
        """Bounded basic SDP, only for standard mono/stereo channel orders.

        Wider layouts remain usable via explicit out-of-band profile agreement.
        Do not publish an ambiguous SDP channel convention for them.
        """
        _endpoint(destination, "udp")
        if self.spec.channels not in (("mono",), ("FL", "FR")):
            raise ValueError("SDP profile requires mono or ordered FL,FR channels")
        return (
            "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=Mixyard RTP\r\n"
            f"c=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {destination.port} RTP/AVP {self.payload_type}\r\n"
            f"a=rtpmap:{self.payload_type} {self.encoding}/{self.spec.sample_rate}/{len(self.spec.channels)}\r\n"
            "a=sendonly\r\n"
        ).encode("ascii")


@dataclass(frozen=True)
class ReceivedPacket:
    spec: Spec
    pcm: bytes
    sequence: int
    timestamp: int
    ssrc: int
    missing_before: int


class _UdpRole:
    role: str

    def _open(self, session: Session, peer: Endpoint, profile: RtpProfile,
              local: Endpoint | None, timeout: float, max_packets: int) -> None:
        _endpoint(peer, "udp")
        if local is not None:
            _endpoint(local, "udp")
        if not isinstance(profile, RtpProfile):
            raise ValueError("RtpProfile required")
        _timeout(timeout)
        _integer(max_packets, 1, MAX_PACKETS, "max_packets")
        self.session, self.peer, self.profile = session, peer, profile
        self.max_packets, self.operations = max_packets, 0
        session.require(self.role, peer, profile.spec)
        self._socket = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            self._socket.settimeout(timeout)
            self._socket.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 16384)
            self._socket.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 16384)
            self._socket.bind(("127.0.0.1", 0 if local is None else local.port))
            self.local = Endpoint("127.0.0.1", self._socket.getsockname()[1], "udp")
        except BaseException:
            self._socket.close()
            raise

    def _admit(self) -> None:
        self.session.require(self.role, self.peer, self.profile.spec)
        if self.operations >= self.max_packets:
            raise RuntimeError("RTP lifetime packet budget exhausted")
        self.operations += 1

    def close(self) -> None:
        self._socket.close()

    def __enter__(self):
        return self

    def __exit__(self, *_exc):
        self.close()


class RtpSender(_UdpRole):
    """Caller-paced sender: each call emits exactly one admitted PCM packet."""

    role = "live-send"

    def __init__(self, session: Session, destination: Endpoint, profile: RtpProfile,
                 *, ssrc: int | None = None, sequence: int | None = None,
                 timestamp: int | None = None, local: Endpoint | None = None,
                 timeout: float = 0.25, max_packets: int = 4096):
        self.ssrc = secrets.randbits(32) if ssrc is None else ssrc
        self.sequence = secrets.randbits(16) if sequence is None else sequence
        self.timestamp = secrets.randbits(32) if timestamp is None else timestamp
        _integer(self.ssrc, 0, 0xFFFFFFFF, "ssrc")
        _integer(self.sequence, 0, 0xFFFF, "sequence")
        _integer(self.timestamp, 0, 0xFFFFFFFF, "timestamp")
        self._open(session, destination, profile, local, timeout, max_packets)

    def send(self, pcm: bytes) -> int:
        payload = self.profile.convert_endian(pcm)
        packet = struct.pack("!BBHII", 0x80, self.profile.payload_type,
                             self.sequence, self.timestamp, self.ssrc) + payload
        self._admit()
        if self._socket.sendto(packet, (self.peer.host, self.peer.port)) != len(packet):
            raise OSError("partial UDP packet")
        sent_sequence = self.sequence
        self.sequence = (self.sequence + 1) & 0xFFFF
        self.timestamp = (self.timestamp + self.profile.spec.frames_per_packet) & 0xFFFFFFFF
        return sent_sequence


class RtpReceiver(_UdpRole):
    """One admitted source/SSRC, no playout queue; rejects duplicate/late media."""

    role = "live-receive"

    def __init__(self, session: Session, local: Endpoint, peer: Endpoint,
                 profile: RtpProfile, *, ssrc: int, timeout: float = 0.25,
                 max_packets: int = 4096):
        _endpoint(local, "udp")
        _integer(ssrc, 0, 0xFFFFFFFF, "ssrc")
        self.ssrc = ssrc
        self._last: tuple[int, int] | None = None
        self.received = self.missing = self.rejected = 0
        self._open(session, peer, profile, local, timeout, max_packets)

    def receive(self) -> ReceivedPacket:
        self._admit()
        packet, address = self._socket.recvfrom(MAX_DATAGRAM + 1)
        # Revocation while blocked must also prevent returning already queued PCM.
        self.session.require(self.role, self.peer, self.profile.spec)
        try:
            if address != (self.peer.host, self.peer.port):
                raise PacketRejected("unadmitted source endpoint")
            if len(packet) != self.profile.payload_bytes + 12:
                raise PacketRejected("wrong packet length or datagram budget")
            flags, pt, seq, timestamp, ssrc = struct.unpack("!BBHII", packet[:12])
            if flags != 0x80 or pt != self.profile.payload_type or ssrc != self.ssrc:
                raise PacketRejected("unsupported RTP header, payload type or SSRC")
            missing = 0
            if self._last is not None:
                last_seq, last_timestamp = self._last
                delta = (seq - last_seq) & 0xFFFF
                if not 1 <= delta <= MAX_GAP:
                    raise PacketRejected("duplicate, late/reordered or excessive sequence gap")
                expected = (last_timestamp + delta * self.profile.spec.frames_per_packet) & 0xFFFFFFFF
                if timestamp != expected:
                    raise PacketRejected("timestamp does not match exact frame count")
                missing = delta - 1
            pcm = self.profile.convert_endian(packet[12:])
            self.session.require(self.role, self.peer, self.profile.spec)
            self._last = (seq, timestamp)
            self.received += 1
            self.missing += missing
            return ReceivedPacket(self.profile.spec, pcm, seq, timestamp, ssrc, missing)
        except PacketRejected:
            self.rejected += 1
            raise
