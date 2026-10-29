"""Synchronous owned-runtime callbacks over separately admitted cleartext RTP."""

from dataclasses import dataclass

from ..runtime import LeasedBlock
from ..core import _identifier
from .rtp import RtpReceiver, RtpSender, _integer
from .rtcp import RtpClockSample


@dataclass(frozen=True)
class _Binding:
    generation: int
    revision: int
    source_epoch: str
    base_frame: int
    clock_id: int

    def __post_init__(self):
        _integer(self.generation, 1, 2**64 - 1, "generation")
        _integer(self.revision, 1, 2**64 - 1, "revision")
        _identifier(self.source_epoch)
        _integer(self.base_frame, 0, 2**64 - 1, "base_frame")
        _integer(self.clock_id, 1, 2**64 - 1, "clock_id")

    def validate(self, block, frame):
        if (block.generation, block.revision, block.source_epoch, block.clock_id) != (
                self.generation, self.revision, self.source_epoch, self.clock_id):
            raise PermissionError("RTP runtime binding changed; explicitly admit a new epoch and SSRC")
        if block.base_frame != frame:
            raise PermissionError("RTP runtime frame discontinuity; no concealment or implicit restart")

    def check_lease(self, lease, spec):
        lease.check()
        b = lease.binding
        if (b.spec, b.generation, b.revision, b.source_epoch, b.clock_id, lease.clock.base_frame) != (
                spec, self.generation, self.revision, self.source_epoch, self.clock_id, self.base_frame):
            raise PermissionError("RTP agreement differs from exact runtime lease and clock origin")


class LeasedRtpSender:
    """One lease/epoch/SSRC; source returns exact immutable LeasedBlocks.

    Caller owns pacing, worker lifecycle, callbacks and the supplied RTP socket.
    No callback is invoked by construction; each send performs live checks.
    """

    def __init__(self, sender: RtpSender, lease, source, *, generation, revision,
                 source_epoch, base_frame, clock_id):
        if not isinstance(sender, RtpSender) or not callable(source):
            raise ValueError("RtpSender and callable source required")
        if sender.sent_packets:
            raise ValueError("fresh RTP sender required for each runtime epoch")
        self.binding = _Binding(generation, revision, source_epoch, base_frame, clock_id)
        self.sender, self.lease, self.source = sender, lease, source
        self._frame, self._timestamp, self._ssrc = base_frame, sender.timestamp, sender.ssrc
        self._sequence = sender.sequence
        self.binding.check_lease(lease, sender.profile.spec)

    def send(self):
        self.lease.check()
        block = self.source()
        if type(block) is not LeasedBlock:
            raise ValueError("source must return an immutable runtime LeasedBlock")
        self.lease.validate(block)
        self.binding.validate(block, self._frame)
        if block.spec != self.sender.profile.spec or len(block.pcm) != self.sender.profile.payload_bytes:
            raise ValueError("exact RTP Spec and full packet PCM required")
        expected = (self._timestamp + self._frame - self.binding.base_frame) & 0xFFFFFFFF
        if (self.sender.ssrc != self._ssrc or self.sender.timestamp != expected
                or self.sender.sequence != self._sequence):
            raise PermissionError("RTP sender was changed outside its runtime epoch")
        self.lease.check()
        sequence = self.lease.accept(block, lambda admitted: self.sender.send(admitted.pcm))
        self._frame += block.frames
        self._sequence = (sequence + 1) & 0xFFFF
        return sequence

    def send_report(self, rtcp, sample: RtpClockSample):
        """Report only an explicitly supplied current common-clock observation."""
        self.lease.check()
        b = self.binding
        if (type(sample) is not RtpClockSample or sample.source_epoch != b.source_epoch
                or sample.clock_id != b.clock_id or sample.base_frame < b.base_frame):
            raise PermissionError("SR needs matching external common-clock and source-epoch evidence")
        timestamp = (self._timestamp + sample.base_frame - b.base_frame) & 0xFFFFFFFF
        rtcp.send_sender_report(self.sender, ntp_timestamp=sample.ntp_timestamp,
                                rtp_timestamp=timestamp)


class LeasedRtpReceiver:
    """Map an explicitly agreed RTP timestamp to a pinned runtime source frame.

    RTP carries no runtime identity, generation, epoch, provenance or clock ID.
    These constructor values are local admission, never learned from packets.
    The runtime sink accepts contiguous media only; loss requires explicit repair.
    """

    def __init__(self, receiver: RtpReceiver, lease, sink, *, generation, revision,
                 source_epoch, base_frame, clock_id, timestamp):
        if not isinstance(receiver, RtpReceiver) or not callable(sink):
            raise ValueError("RtpReceiver and callable sink required")
        if receiver.received:
            raise ValueError("fresh RTP receiver required for each runtime epoch")
        _integer(timestamp, 0, 0xFFFFFFFF, "timestamp")
        self.binding = _Binding(generation, revision, source_epoch, base_frame, clock_id)
        self.receiver, self.lease, self.sink = receiver, lease, sink
        self._timestamp, self._ssrc = timestamp, receiver.ssrc
        self._first_extended = None
        self._frame = base_frame
        self.binding.check_lease(lease, receiver.profile.spec)

    def receive(self):
        self.lease.check()
        if self.receiver.ssrc != self._ssrc:
            raise PermissionError("RTP SSRC changed outside its runtime epoch")
        packet = self.receiver.receive()
        self.lease.check()
        if self._first_extended is None:
            if packet.timestamp != self._timestamp:
                raise PermissionError("first RTP timestamp differs from admitted epoch")
            self._first_extended = packet.extended_timestamp
        frame = self.binding.base_frame + packet.extended_timestamp - self._first_extended
        b = self.binding
        block = LeasedBlock(packet.spec, packet.pcm, b.generation, b.revision,
                            b.source_epoch, frame, b.clock_id)
        self.lease.validate(block)
        b.validate(block, self._frame)
        self.lease.accept(block, self.sink)
        self._frame += block.frames
        return block
