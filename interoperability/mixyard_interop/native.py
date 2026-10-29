"""Optional pinned C ABI host bridge. Python runs only on an owned worker.

Loading a native library is an explicit trusted-code operation, never discovery.
No build, copy, capture, endpoint activation or library search is performed.
The owner must freeze the supplied artifact for the bounded loan lifetime.
"""

import ctypes as C
from collections.abc import Callable
from dataclasses import dataclass, replace
import hashlib
import json
import math
import os
import re
import stat
import struct
import threading

from .runtime import LeasedBlock, RuntimeBusy, RuntimeLease, U64
from .core import Provenance, Spec, _fields, _identifier, _integer

MAX_METADATA = 1024 * 1024
MAX_LIBRARY = 64 * 1024 * 1024


class NativeUnavailable(RuntimeError):
    """No qualified pinned host/provenance boundary is available."""


class NativeRejected(RuntimeError):
    """The actual host rejected a request or block; retained PCM is revoked."""


@dataclass(frozen=True)
class DestinationConversion:
    output: LeasedBlock
    source_spec: Spec
    rounding: str


@dataclass(frozen=True)
class StopWitness:
    """Quiesced/scrubbed evidence for this owned CPU context, not OS endpoints."""
    source_revision: str
    request_sha256: str
    instance_id: int
    generation: int
    revision: int
    committed_frames: int


def convert_processed(block, target_spec, *, consent=False, rounding):
    """Explicit destination-only Float64 -> S16/S24 PCM quantization.

    No clipping, gain, rate/layout change, automatic fallback or sibling mutation.
    Returned evidence describes this narrowing; it is never raw-byte identity.
    """
    if consent is not True:
        raise PermissionError("explicit destination-local integer conversion consent required")
    if (type(block) is not LeasedBlock or type(target_spec) is not Spec or block.spec.codec != "raw-f64-le"
            or target_spec.codec not in {"raw-s16-le", "raw-s24-le"}
            or target_spec != replace(block.spec, codec=target_spec.codec)
            or rounding != "nearest-ties-even"):
        raise ValueError("explicit exact destination format and nearest-ties-even conversion required")
    width = 2 if target_spec.codec == "raw-s16-le" else 3
    scale = 1 << (8 * width - 1)
    packed = bytearray()
    for (sample,) in struct.iter_unpack("<d", block.pcm):
        if not math.isfinite(sample) or not -1 <= sample < 1:
            raise ValueError("processed integer destination range exceeded; no implicit clipping")
        value = round(sample * scale)
        if not -scale <= value < scale:
            raise ValueError("rounded destination range exceeded; no implicit clipping")
        packed.extend(value.to_bytes(width, "little", signed=True))
    output = replace(block, spec=target_spec, pcm=bytes(packed))
    return DestinationConversion(output, block.spec, rounding)


def _strict_json(payload):
    def pairs(entries):
        result = {}
        for key, value in entries:
            if key in result:
                raise ValueError("duplicate native metadata field")
            result[key] = value
        return result
    def constant(_):
        raise ValueError("nonfinite JSON metadata")
    try:
        return json.loads(payload, object_pairs_hook=pairs, parse_constant=constant)
    except (RecursionError, UnicodeError, json.JSONDecodeError) as exc:
        raise ValueError("invalid bounded native metadata") from exc


@dataclass(frozen=True)
class HostLoan:
    path: str
    sha256: str
    source_revision: str
    toolchain: str
    owner_check: Callable[[], bool]

    def __post_init__(self):
        if not isinstance(self.path, str) or not os.path.isabs(self.path):
            raise ValueError("explicit absolute owner library path required")
        if not re.fullmatch(r"[0-9a-f]{64}", self.sha256):
            raise ValueError("exact shared-library SHA256 required")
        if not re.fullmatch(r"[0-9a-f]{40}", self.source_revision):
            raise ValueError("exact owner source revision required")
        if not isinstance(self.toolchain, str) or not 1 <= len(self.toolchain) <= 1024:
            raise ValueError("owner toolchain provenance required")
        if not callable(self.owner_check):
            raise ValueError("explicit live artifact loan check required")

    def check(self):
        if self.owner_check() is not True:
            raise NativeUnavailable("native owner artifact loan no longer active")

    def load(self):
        self.check()
        before = os.stat(self.path, follow_symlinks=False)
        if not stat.S_ISREG(before.st_mode) or not 0 < before.st_size <= MAX_LIBRARY:
            raise NativeUnavailable("bounded regular owner library required")
        digest = hashlib.sha256()
        with open(self.path, "rb") as stream:
            opened = os.fstat(stream.fileno())
            if (opened.st_dev, opened.st_ino) != (before.st_dev, before.st_ino):
                raise NativeUnavailable("owner library changed while opening")
            for chunk in iter(lambda: stream.read(65536), b""):
                digest.update(chunk)
        if digest.hexdigest() != self.sha256:
            raise NativeUnavailable("owner shared-library digest mismatch")
        self.check()
        library = C.CDLL(self.path)
        after = os.stat(self.path, follow_symlinks=False)
        if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (
                after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns):
            raise NativeUnavailable("owner library changed during load")
        return _Abi(library)


class _Stamp(C.Structure):
    _fields_ = [(name, C.c_uint64) for name in (
        "generation", "revision", "clock_id", "sequence", "first_frame", "captured_ns", "deadline_ns")]


class _Input(C.Structure):
    _fields_ = [("bytes", C.POINTER(C.c_uint8)), ("byte_count", C.c_size_t),
                ("encoding", C.c_uint32), ("sample_rate_hz", C.c_uint32),
                ("channels", C.c_uint32), ("stamp", _Stamp)]


class _Packet(C.Structure):
    _fields_ = [("samples", C.POINTER(C.c_double)), ("sample_count", C.c_size_t),
                ("stamp", _Stamp), ("output_index", C.c_uint64),
                ("attestation_sha256", C.c_uint8 * 32)]


class _Abi:
    def __init__(self, library):
        self.lib = library
        signatures = {
            "mx_host_abi_version": ([], C.c_uint32),
            "mx_host_create": ([C.c_void_p, C.c_size_t, C.POINTER(C.c_void_p), C.c_void_p, C.c_size_t], C.c_int),
            "mx_host_destroy": ([C.c_void_p], None),
            "mx_host_revoke": ([C.c_void_p], None),
            "mx_host_source_count": ([C.c_void_p], C.c_size_t),
            "mx_host_output_count": ([C.c_void_p], C.c_size_t),
            "mx_host_process": ([C.c_void_p, C.c_uint64, _Stamp, C.c_size_t, C.POINTER(_Input), C.c_size_t], C.c_int),
            "mx_host_output": ([C.c_void_p, C.c_size_t, C.POINTER(C.POINTER(C.c_double)), C.POINTER(C.c_size_t)], C.c_int),
        }
        for name in ("mx_host_source_info", "mx_host_output_info"):
            signatures[name] = ([C.c_void_p, C.c_size_t, C.c_void_p, C.c_size_t, C.POINTER(C.c_size_t)], C.c_int)
        for name, (args, result) in signatures.items():
            function = getattr(library, name)
            function.argtypes, function.restype = args, result
        if library.mx_host_abi_version() != 1:
            raise NativeUnavailable("unsupported actual host C ABI")

    def info(self, handle, kind, index):
        function = getattr(self.lib, f"mx_host_{kind}_info")
        return _strict_json(self.metadata(function, handle, index))

    def metadata(self, function, handle, index):
        needed = C.c_size_t()
        if function(handle, index, None, 0, C.byref(needed)) != 8 or not 0 < needed.value <= MAX_METADATA:
            raise NativeUnavailable("native metadata unavailable or exceeds complete response budget")
        capacity = needed.value
        buffer = C.create_string_buffer(capacity)
        if function(handle, index, buffer, capacity, C.byref(needed)) != 0 or needed.value != capacity:
            raise NativeUnavailable("native metadata incomplete/changed; no truncation")
        return buffer.raw


_ENCODING = {"raw-f32-le": ("f32-le", 1), "raw-f64-le": ("f64-le", 2),
             "raw-s16-le": ("s16-le", 3), "raw-s24-le": ("s24-le", 4),
             "raw-s32-le": ("s32-le", 5)}
_ATTESTATION_FIELDS = (
    "schemaVersion", "scope", "complete", "processId", "instanceId", "generation", "revision",
    "inventoryRevision", "clockId", "requestSha256", "planSha256", "dspSnapshotSha256",
    "outputIndex", "outputId", "mixId", "output", "contributors")


def _source_spec(lease, info):
    _fields(info, ("observedId", "encoding", "sampleRateHz", "channels", "mapping"))
    if (info["observedId"], info["encoding"], info["sampleRateHz"], info["channels"]) != (
            lease.binding.observed_id, _ENCODING[lease.spec.codec][0], lease.spec.sample_rate,
            list(lease.spec.channels)):
        raise NativeUnavailable("actual host source identity/format differs from owner lease")
    canonical = ["MONO"] if len(lease.spec.channels) == 1 else ["FL", "FR"]
    if info["mapping"] != canonical:
        raise NativeUnavailable("source has no exact admitted canonical channel mapping")


def _output_spec(lease, info):
    _fields(info, ("observedId", "sampleRateHz", "channels", "mapping"))
    if (lease.spec.codec != "raw-f64-le" or
            (info["observedId"], info["sampleRateHz"], info["channels"]) != (
                lease.binding.observed_id, lease.spec.sample_rate, list(lease.spec.channels))):
        raise NativeUnavailable("processed output requires exact observed Float64 destination lease")
    canonical = ["MONO"] if len(lease.spec.channels) == 1 else ["FL", "FR"]
    if info["mapping"] != canonical:
        raise NativeUnavailable("output has no exact admitted canonical channel mapping")


def _provenance(contributors, sources, local_peer):
    """Union only attested dependency paths; never every configured source."""
    source_map, peers = {}, []
    for entry in sorted(contributors, key=lambda value: value["sourceIndex"]):
        index = entry["sourceIndex"]
        lease = sources[index]
        if lease is None:
            raise NativeUnavailable("output contributor lacks preauthorized current source lease")
        for source in lease.spec.provenance.sources:
            key = (source.peer, source.source)
            if key in source_map and source_map[key] != source:
                raise NativeUnavailable("conflicting contributor provenance; reject whole output")
            source_map[key] = source
        for peer in lease.spec.provenance.visited_peers:
            if peer not in peers:
                peers.append(peer)
    if local_peer in peers:
        # Local generators can originate here, but a forwarded path that has
        # already passed through this graph cannot be recaptured.
        for entry in contributors:
            path = sources[entry["sourceIndex"]].spec.provenance.visited_peers
            if local_peer in path and path != (local_peer,):
                raise NativeUnavailable("local graph loop in contributor history")
        peers.remove(local_peer)
    peers.append(local_peer)
    return Provenance(tuple(source_map.values()), tuple(peers))


class NativeHost:
    """One exclusive off-callback owner of an actual compiled C ABI graph.

    Source leases describe preauthorized owner media, not native capture. Only
    the native engine mixes/processes/queues. Packet tokens bind copied Float64
    output to queried per-output conservative contributors and exact stamps.
    Caller-provided now/deadline are on the explicitly qualified common timeline;
    this bridge never turns Python monotonic time into capture time.
    """

    backendAvailable = False
    audioApplied = False

    def __init__(self, loan, request, *, source_leases, output_leases, local_peer, consent=False):
        if consent is not True or type(loan) is not HostLoan:
            raise PermissionError("explicit native owner loan and graph handoff required")
        if type(request) is not bytes or not 0 < len(request) <= MAX_METADATA:
            raise ValueError("bounded exact native request wire bytes required")
        _identifier(local_peer)
        self._request = _strict_json(request)
        self._request_hash = hashlib.sha256(request).hexdigest()
        self._sources, self._outputs = tuple(source_leases), tuple(output_leases)
        if not 1 <= len(self._sources) <= 256 or not 1 <= len(self._outputs) <= 256:
            raise NativeUnavailable("bounded native graph slot budget exceeded")
        if any(l is not None and type(l) is not RuntimeLease for l in self._sources + self._outputs):
            raise ValueError("exact runtime owner leases required")
        if not any(self._outputs):
            raise NativeUnavailable("no admitted processed destination")
        self._loan, self._abi = loan, loan.load()
        lib = self._abi.lib
        if not all(hasattr(lib, name) for name in (
                "mx_host_output_attestation_v1", "mx_host_output_packet_v1")):
            raise NativeUnavailable("native output attestation/packet ABI absent; no egress fallback")
        lib.mx_host_output_attestation_v1.argtypes = [C.c_void_p, C.c_size_t, C.c_void_p, C.c_size_t, C.POINTER(C.c_size_t)]
        lib.mx_host_output_attestation_v1.restype = C.c_int
        lib.mx_host_output_packet_v1.argtypes = [C.c_void_p, C.c_size_t, C.POINTER(_Packet)]
        lib.mx_host_output_packet_v1.restype = C.c_int
        self._handle = C.c_void_p()
        self._lock = threading.Lock()
        self._revoke_requested = threading.Event()
        self._owner_thread = None
        self._active = False
        self._sequence, self._frame = 0, 0
        self._proofs, self._tokens = {}, {}
        self._destination_failures = {}
        self._stop_witness = None
        error = C.create_string_buffer(4096)
        status = lib.mx_host_create(request, len(request), C.byref(self._handle), error, len(error))
        if status != 0 or not self._handle:
            raise NativeRejected(f"actual host admission rejected ({status}): {error.value.decode('utf-8', 'replace')}")
        try:
            if (lib.mx_host_source_count(self._handle), lib.mx_host_output_count(self._handle)) != (
                    len(self._sources), len(self._outputs)):
                raise NativeUnavailable("actual graph slots differ from owner lease slots")
            self._source_info = tuple(self._abi.info(self._handle, "source", i) for i in range(len(self._sources)))
            self._output_info = tuple(self._abi.info(self._handle, "output", i) for i in range(len(self._outputs)))
            for lease, info in zip(self._sources, self._source_info):
                if lease is not None:
                    _source_spec(lease, info)
            budget, instance, snapshot = 0, None, None
            for index, lease in enumerate(self._outputs):
                if lease is None:
                    continue
                _output_spec(lease, self._output_info[index])
                wire = self._abi.metadata(lib.mx_host_output_attestation_v1, self._handle, index)
                budget += len(wire)
                if budget > 4 * MAX_METADATA:
                    raise NativeUnavailable("complete graph provenance metadata budget exceeded")
                proof = _strict_json(wire)
                self._validate_proof(proof, index, local_peer)
                if instance is not None and proof["instanceId"] != instance:
                    raise NativeUnavailable("native output instance changed across queries")
                current = (proof["planSha256"], proof["dspSnapshotSha256"])
                if snapshot is not None and current != snapshot:
                    raise NativeUnavailable("native output graph/DSP snapshot changed")
                instance, snapshot = proof["instanceId"], current
                self._proofs[index], self._tokens[index] = proof, hashlib.sha256(wire).digest()
            self._active = True
            self._check_leases()
        except BaseException:
            lib.mx_host_revoke(self._handle)
            lib.mx_host_destroy(self._handle)
            self._handle = C.c_void_p()
            self._active = False
            raise

    def _validate_proof(self, proof, index, local_peer):
        _fields(proof, _ATTESTATION_FIELDS)
        request = self._request
        if (type(proof["schemaVersion"]) is not int or proof["schemaVersion"] != 1
                or proof["scope"] != "compiled-plan-metadata" or proof["complete"] is not True):
            raise NativeUnavailable("unknown/incomplete native contributor evidence")
        expected = {"generation": request["generation"], "revision": request["config"]["revision"],
                    "inventoryRevision": request["inventory"]["revision"], "clockId": request["clockId"],
                    "outputIndex": index, "processId": os.getpid()}
        for key, value in expected.items():
            if type(proof[key]) is not int or proof[key] != value:
                raise NativeUnavailable(f"native contributor {key} differs from actual request/context")
        _integer(proof["instanceId"], 1, U64, "actual host instance")
        for key in ("requestSha256", "planSha256", "dspSnapshotSha256"):
            if not isinstance(proof[key], str) or not re.fullmatch(r"[0-9a-f]{64}", proof[key]):
                raise NativeUnavailable("missing exact native graph snapshot digest")
        if proof["requestSha256"] != self._request_hash or proof["output"] != self._output_info[index]:
            raise NativeUnavailable("native provenance not bound to exact request/output")
        _identifier(proof["outputId"])
        _identifier(proof["mixId"])
        contributors = proof["contributors"]
        if not isinstance(contributors, list) or not 1 <= len(contributors) <= len(self._sources):
            raise NativeUnavailable("complete nonempty output contributor set required")
        seen = set()
        for entry in contributors:
            _fields(entry, ("sourceIndex", "sourceId", "selector", "origin", "binding"))
            source_index = entry["sourceIndex"]
            _integer(source_index, 0, len(self._sources) - 1, "actual contributor slot")
            if source_index in seen or entry["binding"] != self._source_info[source_index]:
                raise NativeUnavailable("duplicate or unknown native source dependency binding")
            seen.add(source_index)
            _identifier(entry["sourceId"])
            if type(entry["selector"]) is not dict or entry["origin"] != "external":
                raise NativeUnavailable("unknown/recaptured contributor origin")
        if self._outputs[index].spec.provenance != _provenance(contributors, self._sources, local_peer):
            raise NativeUnavailable("processed output provenance differs from complete actual contributor leases")

    def _check_leases(self):
        self._loan.check()
        if not self._active or self._revoke_requested.is_set():
            raise PermissionError("native graph owner revoked")
        anchor = None
        def check_binding(lease):
            lease.check()
            b, clock = lease.binding, lease.clock
            current = (clock.clock_id, clock.base_frame, clock.base_time_ns, clock.qualification,
                       lease.spec.sample_rate)
            if (b.generation, b.revision, b.clock_id) != (
                    self._request["generation"], self._request["config"]["revision"], self._request["clockId"]):
                raise NativeUnavailable("host generation/revision/clock differs from owner lease")
            if clock.base_frame != 0:
                raise NativeUnavailable("this host ABI requires explicit epoch frame zero")
            return current
        for lease in self._sources:
            if lease is not None:
                current = check_binding(lease)
                if anchor is not None and current != anchor:
                    raise NativeUnavailable("sources lack exact qualified common timeline")
                anchor = current
        for index, lease in enumerate(self._outputs):
            if lease is None or index in self._destination_failures:
                continue
            try:
                if check_binding(lease) != anchor:
                    raise NativeUnavailable("destination lacks exact qualified common timeline")
            except Exception as exc:
                self._destination_failures[index] = str(exc)
        # Final pure liveness checks catch cross-lease revocation by the last
        # external callback. Source loss invalidates the entire graph generation.
        for lease in self._sources:
            if lease is not None:
                lease.check_liveness()
        for index, lease in enumerate(self._outputs):
            if lease is not None and index not in self._destination_failures:
                try:
                    lease.check_liveness()
                except PermissionError as exc:
                    self._destination_failures[index] = str(exc)
        if self._revoke_requested.is_set():
            raise PermissionError("native graph revoked during owner observation")
        if not any(lease is not None and i not in self._destination_failures
                   for i, lease in enumerate(self._outputs)):
            raise NativeUnavailable("no current processed destinations remain")

    def process(self, blocks, *, now_ns, deadline_ns):
        if not self._lock.acquire(blocking=False):
            raise RuntimeBusy("actual native graph worker busy")
        self._owner_thread = threading.get_ident()
        try:
            self._check_leases()
            expected = {i for i, lease in enumerate(self._sources) if lease is not None}
            if type(blocks) is not dict or set(blocks) != expected:
                raise NativeUnavailable("missing/unknown source PCM; silence is not captured media")
            frames, captured = None, None
            inputs, storage = (_Input * len(self._sources))(), []
            _integer(now_ns, 0, U64, "qualified owner timeline now")
            _integer(deadline_ns, now_ns, U64, "qualified owner deadline")
            for index in sorted(expected):
                block, lease = blocks[index], self._sources[index]
                lease.validate(block)
                time_ns = lease.clock.time_ns(block.base_frame, block.spec.sample_rate)
                if block.base_frame != self._frame or (frames is not None and frames != block.frames):
                    raise NativeUnavailable("source block gap/repeat/partial common-clock interval")
                if captured is not None and captured != time_ns:
                    raise NativeUnavailable("source capture anchors differ on common timeline")
                frames, captured = block.frames, time_ns
                if not captured <= now_ns <= deadline_ns:
                    raise NativeUnavailable("source common-clock interval expired or in future")
                data = (C.c_uint8 * len(block.pcm)).from_buffer_copy(block.pcm)
                storage.append(data)
                stamp = _Stamp(block.generation, block.revision, block.clock_id, self._sequence,
                               block.base_frame, captured, deadline_ns)
                inputs[index] = _Input(data, len(block.pcm), _ENCODING[block.spec.codec][1],
                                       block.spec.sample_rate, len(block.spec.channels), stamp)
            if frames is None or frames > self._request["maxFrames"]:
                raise NativeUnavailable("no complete bounded source interval")
            stamp = _Stamp(self._request["generation"], self._request["config"]["revision"],
                           self._request["clockId"], self._sequence, self._frame, captured, deadline_ns)
            self._check_leases()
            status = self._abi.lib.mx_host_process(self._handle, now_ns, stamp, frames, inputs, len(inputs))
            if status != 0:
                raise NativeRejected(f"actual host process rejected ({status})")
            result = {}
            for index, lease in enumerate(self._outputs):
                if lease is None or index in self._destination_failures:
                    continue
                packet = _Packet()
                status = self._abi.lib.mx_host_output_packet_v1(self._handle, index, C.byref(packet))
                if status == 7:
                    self._destination_failures[index] = "actual native output inactive"
                    continue
                if (status != 0 or not packet.samples or packet.sample_count != frames * len(lease.spec.channels)
                        or packet.output_index != index or bytes(packet.attestation_sha256) != self._tokens[index]
                        or bytes(packet.stamp) != bytes(stamp)):
                    raise NativeUnavailable("native packet lacks exact instance snapshot/stamp/contributor token")
                samples = packet.samples[:packet.sample_count]
                if not all(math.isfinite(value) for value in samples):
                    self._destination_failures[index] = "actual processed output contains nonfinite samples"
                    continue
                pcm = struct.pack(f"<{len(samples)}d", *samples)
                b = lease.binding
                block = LeasedBlock(lease.spec, pcm, b.generation, b.revision, b.source_epoch,
                                    self._frame, b.clock_id)
                try:
                    lease.validate(block)
                except Exception as exc:
                    self._destination_failures[index] = str(exc)
                    continue
                result[index] = block
            # An external revoke is a signal only. This exclusive owner performs
            # C revoke and discards all copied PCM before returning to any sink.
            self._check_leases()
            result = {i: block for i, block in result.items() if i not in self._destination_failures}
            self._sequence += 1
            self._frame += frames
            return result
        except BaseException:
            self._revoke_requested.set()
            if self._handle:
                self._abi.lib.mx_host_revoke(self._handle)
            self._active = False
            raise
        finally:
            self._owner_thread = None
            self._lock.release()

    def output_proof(self, index):
        # Return an independent value, never a mutable view of token-bound state.
        return _strict_json(json.dumps(self._proofs[index]).encode("utf-8"))

    def revoke(self):
        self._revoke_requested.set()

    def source_lost(self, index):
        """Actual owner signal: discard this whole bridge generation at commit."""
        _integer(index, 0, len(self._sources) - 1, "lost source slot")
        if self._sources[index] is None:
            raise ValueError("source slot has no owned lease")
        self.revoke()
        self._sources[index].source_lost()

    @property
    def destination_failures(self):
        return dict(self._destination_failures)

    @property
    def stop_witness(self):
        return self._stop_witness

    def quiesce(self, *, timeout_seconds=1.0):
        """Signal stop, await exclusive ownership, scrub and verify no packets.

        A busy/failed stop preserves this context and ownership. It grants no
        permission to destroy, release an artifact loan, rebind or autoresume.
        Native endpoint owners require their own independent stop witness.
        """
        if (type(timeout_seconds) not in (int, float) or not math.isfinite(timeout_seconds)
                or not 0 <= timeout_seconds <= 2):
            raise ValueError("bounded quiesce timeout required")
        self.revoke()
        if self._owner_thread == threading.get_ident():
            raise RuntimeBusy("native owner callback has not quiesced")
        if not self._lock.acquire(timeout=timeout_seconds):
            raise RuntimeBusy("native graph stop incomplete; retain context and loan ownership")
        try:
            if not self._handle:
                return self._stop_witness
            self._abi.lib.mx_host_revoke(self._handle)
            self._active = False
            for index in range(len(self._outputs)):
                packet = _Packet()
                status = self._abi.lib.mx_host_output_packet_v1(self._handle, index, C.byref(packet))
                if status != 7 or any(bytes(packet)):
                    raise NativeUnavailable("native scrub completion unproved; retain context ownership")
            instance = next(iter(self._proofs.values()))["instanceId"]
            self._stop_witness = StopWitness(self._loan.source_revision, self._request_hash, instance,
                                             self._request["generation"], self._request["config"]["revision"],
                                             self._frame)
            return self._stop_witness
        finally:
            self._lock.release()

    def close(self):
        witness = self.quiesce()
        with self._lock:
            if self._handle:
                self._abi.lib.mx_host_destroy(self._handle)
                self._handle = C.c_void_p()
            self._active = False
        return witness

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()
