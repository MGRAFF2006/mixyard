"""Explicit leased PCM callbacks over paced, bounded HTTP/1.1 chunked records.

This private bilateral profile carries timeline evidence, not hardware timestamps.
Callbacks run on an ordinary worker and must return promptly; never use a native
audio callback here. No queues, listeners or URL fetches start on import.
"""
from __future__ import annotations

import json
import select
import socket
import threading
import time
from dataclasses import dataclass

from mixyard_interop.core import Endpoint, Spec

from .http import (MAX_HEADER, ProtocolError, Resource, _head, _message,
                   _spec_header, endpoint_checked)

MAX_PCM = 65536
MAX_METADATA = 1024
MAX_RECORD = 4 + MAX_METADATA + MAX_PCM
IO_TIMEOUT = 2.0
POLL = 0.05
PROFILE = "leased-pcm-v1"
_FIELDS = ("generation", "revision", "source_epoch", "base_frame", "clock_id")


@dataclass(frozen=True)
class StreamResult:
    blocks: int
    frames: int
    pcm_bytes: int


class _Socket:
    """Small socket adapter giving the shared header parser cancellation checks."""
    def __init__(self, sock, check):
        self.sock, self.check = sock, check
        self.deadline = time.monotonic() + IO_TIMEOUT

    def settimeout(self, timeout):
        # _head passes its remaining absolute budget, not a fresh deadline.
        self.sock.settimeout(min(POLL, timeout))

    def recv(self, size):
        while True:
            self.check()
            remaining = self.deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("stream IO deadline")
            self.sock.settimeout(min(POLL, remaining))
            try:
                return self.sock.recv(size)
            except socket.timeout:
                continue

    def send(self, data):
        while True:
            self.check()
            remaining = self.deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("stream IO deadline")
            self.sock.settimeout(min(POLL, remaining))
            try:
                return self.sock.send(data)
            except socket.timeout:
                continue

    def fresh(self):
        self.deadline = time.monotonic() + IO_TIMEOUT

    def read(self, size):
        data = bytearray()
        while len(data) < size:
            part = self.recv(min(4096, size - len(data)))
            if not part:
                raise ProtocolError("incomplete stream record")
            data.extend(part)
        self.check()
        return bytes(data)

    def write(self, data):
        view = memoryview(data)
        while view:
            sent = self.send(view[:4096])
            if not sent:
                raise ProtocolError("closed during stream write")
            view = view[sent:]
        self.check()


def _metadata(value):
    if type(value) is not dict or set(value) != set(_FIELDS):
        raise ProtocolError("exact leased block metadata required")
    for field in ("generation", "revision", "base_frame", "clock_id"):
        low = 0 if field == "base_frame" else 1
        if type(value[field]) is not int or not low <= value[field] < 2**64:
            raise ProtocolError("invalid leased block integer")
    epoch = value["source_epoch"]
    if (not isinstance(epoch, str) or not epoch or len(epoch.encode("utf-8")) > 128
            or any(ord(c) < 32 or 127 <= ord(c) <= 159 for c in epoch)):
        raise ProtocolError("invalid source epoch")
    return value


def _pcm(spec, pcm):
    if (type(pcm) is not bytes or not pcm or len(pcm) > MAX_PCM
            or len(pcm) % spec.frame_bytes
            or len(pcm) // spec.frame_bytes > spec.frames_per_packet):
        raise ProtocolError("bounded whole leased PCM frames required")


def _record(spec, block):
    if block.spec != spec:
        raise ProtocolError("source block Spec changed")
    _pcm(spec, block.pcm)
    meta = _metadata({field: getattr(block, field) for field in _FIELDS})
    if meta["base_frame"] + len(block.pcm) // spec.frame_bytes >= 2**64:
        raise ProtocolError("block frame range overflow")
    payload = json.dumps(meta, ensure_ascii=True, separators=(",", ":")).encode("ascii")
    if len(payload) > MAX_METADATA:
        raise ProtocolError("leased metadata budget")
    return len(payload).to_bytes(4, "big") + payload + block.pcm


def _unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ProtocolError("duplicate leased metadata key")
        result[key] = value
    return result


def _decode(spec, record, block_factory):
    if len(record) < 4:
        raise ProtocolError("missing leased metadata length")
    size = int.from_bytes(record[:4], "big")
    if not 1 <= size <= MAX_METADATA or 4 + size >= len(record):
        raise ProtocolError("leased metadata/PCM bounds")
    try:
        meta = _metadata(json.loads(record[4:4 + size], object_pairs_hook=_unique))
    except (UnicodeError, ValueError, RecursionError) as exc:
        raise ProtocolError("invalid leased metadata") from exc
    pcm = record[4 + size:]
    _pcm(spec, pcm)
    if meta["base_frame"] + len(pcm) // spec.frame_bytes >= 2**64:
        raise ProtocolError("block frame range overflow")
    return block_factory(spec=spec, pcm=pcm, **meta)


def _chunk(io):
    # No extensions, trailers, whitespace or body-sized integer/parser buffers.
    line = bytearray()
    while not line.endswith(b"\r\n"):
        line.extend(io.read(1))
        if len(line) > 10:
            raise ProtocolError("stream chunk size line budget")
    raw = line[:-2]
    if not raw or any(c not in b"0123456789abcdefABCDEF" for c in raw):
        raise ProtocolError("invalid stream chunk size")
    size = int(raw, 16)
    if size > MAX_RECORD:
        raise ProtocolError("stream record budget")
    data = io.read(size)
    if io.read(2) != b"\r\n":
        raise ProtocolError("stream chunk terminator/trailers")
    return data


class StreamSource:
    """One owned listener and active stream, no retained PCM or growing queue.

    read_block() -> LeasedBlock | None; check_lease() and validate_block(block)
    must be bound to the same runtime lease. None cleanly finishes this response.
    A second request is not an implicit runtime rewind or a new lease.
    """
    def __init__(self, session, spec: Spec, read_block, *, check_lease,
                 validate_block, endpoint: Endpoint):
        endpoint_checked(endpoint)
        if type(spec) is not Spec:
            raise TypeError("canonical Spec required")
        if not all(callable(cb) for cb in (read_block, check_lease, validate_block)):
            raise TypeError("explicit runtime lease callbacks required")
        self.session, self.spec, self.endpoint = session, spec, endpoint
        self.read_block, self.check_lease, self.validate_block = read_block, check_lease, validate_block
        self.resource = Resource(endpoint, "/stream", spec)
        self._stop = threading.Event()
        self._lock = threading.Lock()
        self._active = None
        self.errors = 0
        self.completed = 0
        self.last_error = None
        self._sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:
            self._check()
            self._sock.bind((endpoint.host, endpoint.port))
            self._sock.listen(2)
            self._sock.settimeout(POLL)
            self._thread = threading.Thread(target=self._run, daemon=True)
            self._thread.start()
        except BaseException:
            self._stop.set()
            self._sock.close()
            raise

    def _check(self):
        if self._stop.is_set():
            raise PermissionError("stream source closed")
        self.check_lease()
        self.session.require("live-send", self.endpoint, self.spec)

    def _serve(self, sock):
        pending = None

        def check():
            self._check()
            if pending is not None:
                self.validate_block(pending)
            self.session.require("live-send", self.endpoint, self.spec)

        io = _Socket(sock, check)
        first, headers, length = _head(io, io.deadline, check, request_head=True)
        if (first != "GET /stream HTTP/1.1" or length != 0
                or headers.get("host") != f"127.0.0.1:{self.endpoint.port}"):
            raise ProtocolError("invalid stream request")
        head = ("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n"
                "Content-Type: application/vnd.mixyard.pcm\r\n"
                f"X-Mixyard-Stream: {PROFILE}\r\nX-Mixyard-Spec: {_spec_header(self.spec)}\r\n\r\n").encode("ascii")
        if len(head) > MAX_HEADER:
            raise ProtocolError("stream header budget")
        io.write(head)
        due = time.monotonic()
        while True:
            # Poll disconnect/revoke during pacing. Never burst to catch up after
            # a slow producer or blocked consumer, nor call a tick hardware time.
            while True:
                check()
                readable, _, _ = select.select([sock], [], [], 0)
                if readable:
                    if not sock.recv(1, socket.MSG_PEEK):
                        raise ConnectionError("stream peer disconnected")
                    raise ProtocolError("unexpected request bytes")
                left = due - time.monotonic()
                if left <= 0:
                    break
                self._stop.wait(min(POLL, left))
            callback_deadline = time.monotonic() + IO_TIMEOUT
            pending = self.read_block()
            check()
            if time.monotonic() >= callback_deadline:
                raise TimeoutError("source callback deadline")
            io.fresh()
            if pending is None:
                io.write(b"0\r\n\r\n")
                return
            record = _record(self.spec, pending)
            io.write(f"{len(record):X}\r\n".encode("ascii"))
            io.write(record)
            io.write(b"\r\n")
            due = time.monotonic() + len(pending.pcm) / self.spec.frame_bytes / self.spec.sample_rate
            pending = None

    def _run(self):
        try:
            while not self._stop.is_set():
                try:
                    self._check()
                    sock, _ = self._sock.accept()
                except socket.timeout:
                    continue
                except Exception as exc:
                    self.errors += 1
                    self.last_error = type(exc).__name__
                    break
                with sock:
                    with self._lock:
                        self._active = sock
                    try:
                        self._serve(sock)
                        self.completed += 1
                    except Exception as exc:
                        # Keep a bounded error kind only; no callback/peer text.
                        self.errors += 1
                        self.last_error = type(exc).__name__
                    finally:
                        with self._lock:
                            self._active = None
        finally:
            self._sock.close()

    def close(self):
        self._stop.set()
        with self._lock:
            if self._active is not None:
                try:
                    self._active.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass  # The peer may already have disconnected.
        self._thread.join(IO_TIMEOUT + POLL)
        if self._thread.is_alive():
            raise RuntimeError("runtime source callback did not stop promptly")

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()


class StreamReceiver:
    """Synchronous owned receive worker; backpressure is the bounded callback.

    accept_block must be the destination RuntimeLease.accept(block, consumer).
    write_block is the bounded consumer, without another admission callback.
    The protocol grant is rechecked inside accept, after owner validation and
    immediately before the consumer. No unbounded queue may be retained.
    cancel is an optional caller-owned threading.Event. Cancellation/expiry raises
    PermissionError; disconnect/truncation raises ProtocolError or OSError.
    """
    def __init__(self, session):
        self.session = session
        self._operation = threading.Lock()

    def receive(self, resource, write_block, *, check_lease, accept_block, block_factory, cancel=None):
        if not self._operation.acquire(blocking=False):
            raise RuntimeError("HTTP receive worker busy; no connection or record consumed")
        try:
            return self._receive(resource, write_block, check_lease=check_lease,
                                 accept_block=accept_block, block_factory=block_factory, cancel=cancel)
        finally:
            self._operation.release()

    def _receive(self, resource, write_block, *, check_lease, accept_block, block_factory, cancel):
        if resource.wire != "raw":
            raise ProtocolError("leased streams require exact raw PCM")
        if not all(callable(cb) for cb in (write_block, check_lease, accept_block, block_factory)):
            raise TypeError("explicit runtime lease callbacks required")
        cancel = cancel if cancel is not None else threading.Event()

        def check():
            if cancel.is_set():
                raise PermissionError("stream receive cancelled")
            check_lease()
            self.session.require("live-receive", resource.endpoint, resource.spec)

        check()
        blocks = frames = pcm_bytes = 0
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            sock.settimeout(POLL)
            # Literal loopback connect has no resolver/proxy or discovered URL.
            sock.connect((resource.endpoint.host, resource.endpoint.port))
            io = _Socket(sock, check)
            io.write(_message(f"GET {resource.path} HTTP/1.1",
                             {"Host": f"127.0.0.1:{resource.endpoint.port}"}, b""))
            first, headers, _ = _head(io, io.deadline, check, chunked=True)
            if (first != "HTTP/1.1 200 OK" or headers.get("x-mixyard-stream") != PROFILE
                    or headers.get("content-type") != "application/vnd.mixyard.pcm"
                    or headers.get("x-mixyard-spec") != _spec_header(resource.spec)):
                raise ProtocolError("exact admitted stream profile/Spec required")
            while True:
                io.fresh()
                record = _chunk(io)
                if not record:
                    check()
                    return StreamResult(blocks, frames, pcm_bytes)
                block = _decode(resource.spec, record, block_factory)
                check()
                callback_deadline = time.monotonic() + IO_TIMEOUT
                def deliver(admitted):
                    if cancel.is_set():
                        raise PermissionError("stream receive cancelled before consumer")
                    self.session.require("live-receive", resource.endpoint, resource.spec)
                    return write_block(admitted)
                accept_block(block, deliver)
                check()
                if time.monotonic() >= callback_deadline:
                    raise TimeoutError("consumer callback deadline; delivery may have occurred")
                blocks += 1
                frames += len(block.pcm) // resource.spec.frame_bytes
                pcm_bytes += len(block.pcm)
