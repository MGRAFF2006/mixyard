"""Explicit owner leases for off-callback protocol workers.

This module neither captures PCM nor grants OS/device access. An already
authorized runtime owner supplies the current binding on every admission.
No queue, mixer, resampler, or hardware clock is implemented here.
"""

from dataclasses import dataclass
import math
import threading
import time

from .core import Spec, _identifier, _integer

MAX_BLOCK_BYTES = 1024 * 1024
U64 = 2**64 - 1


class RuntimeBusy(RuntimeError):
    """A worker attempted to reenter an exclusively owned operation."""


@dataclass(frozen=True)
class ClockEvidence:
    clock_id: int
    source_epoch: str
    base_frame: int
    base_time_ns: int
    qualification: str = "owned-synthetic-common-clock"

    def __post_init__(self):
        _integer(self.clock_id, 1, U64, "common clock ID")
        _identifier(self.source_epoch)
        _integer(self.base_frame, 0, U64, "base frame")
        _integer(self.base_time_ns, 0, U64, "base timeline time")
        if self.qualification not in {"owned-synthetic-common-clock", "qualified-owner-common-clock"}:
            raise ValueError("explicit common-clock qualification required")

    def time_ns(self, frame, sample_rate):
        _integer(frame, self.base_frame, U64, "timeline frame")
        _integer(sample_rate, 1, 2**32 - 1, "sample rate")
        value = self.base_time_ns + (frame - self.base_frame) * 1_000_000_000 // sample_rate
        _integer(value, 0, U64, "timeline time")
        return value


@dataclass(frozen=True)
class LeasedBlock:
    spec: Spec
    pcm: bytes
    generation: int
    revision: int
    source_epoch: str
    base_frame: int
    clock_id: int

    def __post_init__(self):
        if type(self.spec) is not Spec or type(self.pcm) is not bytes:
            raise ValueError("canonical Spec and immutable complete PCM bytes required")
        if (not self.pcm or len(self.pcm) > MAX_BLOCK_BYTES
                or len(self.pcm) % self.spec.frame_bytes
                or self.frames > self.spec.frames_per_packet):
            raise ValueError("bounded complete PCM block required; no partial/silent padding")
        for name in ("generation", "revision", "clock_id"):
            _integer(getattr(self, name), 1, U64, name)
        _identifier(self.source_epoch)
        _integer(self.base_frame, 0, U64 - self.frames, "block frame")

    @property
    def frames(self):
        return len(self.pcm) // self.spec.frame_bytes


@dataclass(frozen=True)
class LeaseBinding:
    observed_id: str
    spec: Spec
    generation: int
    revision: int
    source_epoch: str
    clock_id: int
    clock_evidence: ClockEvidence

    def __post_init__(self):
        _identifier(self.observed_id)
        _identifier(self.source_epoch)
        if type(self.spec) is not Spec:
            raise ValueError("exact observed Spec required")
        for name in ("generation", "revision", "clock_id"):
            _integer(getattr(self, name), 1, U64, name)
        if (type(self.clock_evidence) is not ClockEvidence
                or self.clock_evidence.clock_id != self.clock_id
                or self.clock_evidence.source_epoch != self.source_epoch):
            raise ValueError("owner binding must attest the complete source clock mapping")


class RuntimeLease:
    """Ephemeral owner handoff, independent of directional network Session grants.

    owner_check returns the owner's current LeaseBinding, or raises when its
    preexisting authority is unavailable. Construction is not capture consent.
    validate is idempotent; accept additionally enforces contiguous frames and
    advances only after a successful synchronous consumer. Callbacks must be
    bounded and off the audio callback. Revocation cannot recall a completed IO.
    """

    backendAvailable = False
    audioApplied = False

    def __init__(self, spec, *, observed_id, generation, revision, source_epoch,
                 clock, owner_check, consent=False, lifetime_seconds=60,
                 excluded_groups=()):
        if consent is not True:
            raise PermissionError("explicit preauthorized owner lease handoff required")
        if type(clock) is not ClockEvidence or clock.source_epoch != source_epoch:
            raise ValueError("source epoch and explicit common-clock evidence required")
        if not callable(owner_check):
            raise ValueError("current runtime owner observation callback required")
        if (type(lifetime_seconds) not in (int, float) or not math.isfinite(lifetime_seconds)
                or not 0 < lifetime_seconds <= 3600):
            raise ValueError("finite owner lease lifetime at most one hour required")
        excluded = tuple(excluded_groups)
        for group in excluded:
            _identifier(group)
        if type(spec) is not Spec:
            raise ValueError("canonical observed Spec required")
        if any(set(s.groups).intersection(excluded) for s in spec.provenance.sources):
            raise PermissionError("excluded source in premix; reject whole lease")
        self._binding = LeaseBinding(observed_id, spec, generation, revision, source_epoch,
                                     clock.clock_id, clock)
        self._clock = clock
        self._owner_check = owner_check
        self._deadline = time.monotonic() + lifetime_seconds
        self._active = True
        self._next_frame = clock.base_frame
        self._lock = threading.RLock()
        self._busy = False
        self._checking = False
        self.check()

    @property
    def binding(self):
        return self._binding

    @property
    def spec(self):
        return self._binding.spec

    @property
    def clock(self):
        return self._clock

    def _check_locked(self):
        if not self._active or time.monotonic() >= self._deadline:
            raise PermissionError("runtime owner lease revoked or expired")
        if self._checking:
            self._active = False
            raise RuntimeBusy("runtime owner observation reentry")
        self._checking = True
        try:
            current = self._owner_check()
        except BaseException:
            self._active = False
            raise
        finally:
            self._checking = False
        # Owner code may revoke reentrantly on this RLock or run past expiry.
        # No downstream consumer may be admitted on that callback's return.
        if not self._active or time.monotonic() >= self._deadline:
            raise PermissionError("runtime owner lease revoked or expired during observation")
        if current != self._binding:
            self._active = False
            raise PermissionError("runtime owner identity/format/generation/revision/epoch/clock mapping changed")

    def check(self):
        with self._lock:
            self._check_locked()

    def validate(self, block):
        with self._lock:
            self._check_locked()
            b = self._binding
            if (type(block) is not LeasedBlock or block.spec != b.spec
                    or (block.generation, block.revision, block.source_epoch, block.clock_id)
                    != (b.generation, b.revision, b.source_epoch, b.clock_id)
                    or block.base_frame < self.clock.base_frame):
                raise PermissionError("block differs from exact runtime owner lease")
            self.clock.time_ns(block.base_frame + block.frames, block.spec.sample_rate)

    def read(self, source):
        self.check()
        block = source()
        self.check()
        if block is not None:
            self.validate(block)
        return block

    def accept(self, block, consumer):
        # Do not wait behind a slow sink and later admit a stale queued block.
        if not self._lock.acquire(blocking=False):
            raise RuntimeBusy("runtime lease consumer busy")
        try:
            if self._busy:
                raise RuntimeBusy("runtime lease consumer reentry")
            self.validate(block)
            if block.base_frame != self._next_frame:
                raise PermissionError("missing/repeated PCM; fresh epoch lease required")
            self._busy = True
            try:
                result = consumer(block)
                self._check_locked()
                self._next_frame += block.frames
                return result
            except BaseException:
                # A callback may have consumed a prefix before failing. Retrying
                # would falsely report exact continuous delivery.
                self._active = False
                raise
            finally:
                self._busy = False
        finally:
            self._lock.release()

    def revoke(self):
        with self._lock:
            self._active = False

