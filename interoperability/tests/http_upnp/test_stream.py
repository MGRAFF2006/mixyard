"""Continuous production HTTP workers, synthetic data and owned loopback only."""
import hashlib
import http.client
import json
import socket
import threading
import time
import unittest
from contextlib import contextmanager
from dataclasses import dataclass, replace
from unittest.mock import patch

from mixyard_interop.core import Endpoint, Grant, Session, Spec
from mixyard_interop.http_upnp import ProtocolError, Resource, StreamReceiver, StreamSource
from mixyard_interop.http_upnp.http import MAX_MEDIA, _spec_header
from mixyard_interop.http_upnp.stream import MAX_RECORD, PROFILE


def endpoint():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return Endpoint("127.0.0.1", sock.getsockname()[1])


def spec(codec="raw-s24-le", *, rate=1000, frames=20, channels=("FR", "FL", "AUX")):
    return Spec.from_dict({"stream": 37, "name": "leased owned source", "codec": codec,
                          "sample_rate": rate, "channels": list(channels),
                          "frames_per_packet": frames,
                          "provenance": {"sources": [{"peer": "origin", "source": "fixture", "groups": []}],
                                         "visited_peers": ["origin"]}})


def admit(block, consumer):
    """Transport-only double; actual RuntimeLease tests use lease.accept."""
    return consumer(block)


def sessions(fmt, ep):
    return (Session("origin", [Grant("live-send", ep, "sink", fmt)], consent=True),
            Session("sink", [Grant("live-receive", ep, "origin", fmt)], consent=True))


@dataclass(frozen=True)
class Block:
    """Transport contract double; actual runtime leases are exercised separately."""
    spec: Spec
    pcm: bytes
    generation: int = 3
    revision: int = 5
    source_epoch: str = "owned-epoch"
    base_frame: int = 0
    clock_id: int = 7


def block(fmt, base=0):
    pcm = bytes((i * 53 + base) % 256 for i in range(fmt.frame_bytes * fmt.frames_per_packet))
    return Block(fmt, pcm, base_frame=base)


def checked():
    pass


def validated(block):
    pass


def response_head(fmt, *, extra=b"", first=b"HTTP/1.1 200 OK", profile=PROFILE):
    return (first + b"\r\nTransfer-Encoding: chunked\r\n"
            b"Content-Type: application/vnd.mixyard.pcm\r\n"
            + f"X-Mixyard-Stream: {profile}\r\nX-Mixyard-Spec: {_spec_header(fmt)}\r\n".encode()
            + extra + b"\r\n")


def metadata(**changes):
    return {"generation": 3, "revision": 5, "source_epoch": "owned-epoch",
            "base_frame": 0, "clock_id": 7, **changes}


def wire_record(fmt, meta=None, pcm=None):
    meta = json.dumps(metadata() if meta is None else meta).encode() if not isinstance(meta, bytes) else meta
    data = len(meta).to_bytes(4, "big") + meta + (block(fmt).pcm if pcm is None else pcm)
    return f"{len(data):x}\r\n".encode() + data + b"\r\n"


@contextmanager
def peer(response, *, after_send=None):
    """Bounded adversarial peer; only the response is replaced, not our receiver."""
    with socket.socket() as server:
        server.bind(("127.0.0.1", 0))
        server.listen(1)
        server.settimeout(2)
        ep = Endpoint("127.0.0.1", server.getsockname()[1])
        failures = []

        def run():
            try:
                conn, _ = server.accept()
                with conn:
                    conn.settimeout(2)
                    request = bytearray()
                    while not request.endswith(b"\r\n\r\n") and len(request) < 8192:
                        part = conn.recv(1)
                        if not part:
                            return
                        request.extend(part)
                    conn.sendall(response)
                    if after_send:
                        after_send(conn)
            except (BrokenPipeError, ConnectionResetError):
                pass  # Rejection may close before this synthetic peer finishes.
            except Exception as exc:
                failures.append(exc)

        worker = threading.Thread(target=run)
        worker.start()
        try:
            yield ep
        finally:
            worker.join(3)
            if worker.is_alive():
                raise AssertionError("owned peer failed to stop")
            if failures:
                raise failures[0]


class StreamTests(unittest.TestCase):
    def receive(self, fmt, ep, sink, **kwargs):
        _, session = sessions(fmt, ep)
        return StreamReceiver(session).receive(Resource(ep, "/stream", fmt), sink,
                                               check_lease=checked, accept_block=admit, block_factory=Block, **kwargs)

    def test_unbounded_total_stream_preserves_all_raw_profiles_without_accumulation(self):
        for codec in ("raw-s16-le", "raw-s24-le", "raw-s32-le", "raw-f32-le", "raw-f64-le"):
            with self.subTest(codec=codec):
                fmt = spec(codec, rate=96000, frames=128,
                           channels=tuple(f"ordered-{i}" for i in range(64)))
                ep = endpoint()
                send, receive = sessions(fmt, ep)
                expected, actual = hashlib.sha256(), hashlib.sha256()
                next_frame = 0
                count = MAX_MEDIA // (fmt.frame_bytes * fmt.frames_per_packet) + 2
                produced = 0

                def read():
                    nonlocal produced
                    if produced == count:
                        return None
                    result = block(fmt, produced * fmt.frames_per_packet)
                    expected.update(result.pcm)
                    produced += 1
                    return result

                def consume(value):
                    nonlocal next_frame
                    self.assertEqual(value.spec, fmt)
                    self.assertEqual((value.generation, value.revision, value.source_epoch, value.clock_id),
                                     (3, 5, "owned-epoch", 7))
                    self.assertEqual(value.base_frame, next_frame)
                    next_frame += fmt.frames_per_packet
                    actual.update(value.pcm)

                with StreamSource(send, fmt, read, check_lease=checked,
                                  validate_block=validated, endpoint=ep) as source:
                    result = StreamReceiver(receive).receive(source.resource, consume,
                                                            check_lease=checked, accept_block=admit, block_factory=Block)
                    self.assertEqual(source.errors, 0)
                self.assertEqual(result.blocks, count)
                self.assertEqual(result.frames, next_frame)
                self.assertGreater(result.pcm_bytes, MAX_MEDIA)
                self.assertEqual(actual.digest(), expected.digest())

    def test_cadence_and_slow_producer_never_catch_up_with_bursts(self):
        fmt, ep = spec(), endpoint()
        send, receive = sessions(fmt, ep)
        arrivals = []
        produced = 0

        def read():
            nonlocal produced
            if produced == 5:
                return None
            if produced == 2:
                threading.Event().wait(0.08)  # Synthetic source stall.
            result = block(fmt, produced * 20)
            produced += 1
            return result

        with StreamSource(send, fmt, read, check_lease=checked,
                          validate_block=validated, endpoint=ep) as source:
            StreamReceiver(receive).receive(source.resource, lambda b: arrivals.append(time.monotonic()),
                                            check_lease=checked, accept_block=admit, block_factory=Block)
        self.assertEqual(len(arrivals), 5)
        self.assertGreaterEqual(arrivals[-1] - arrivals[0], 0.15)
        # The block after the delayed block still waits a frame period.
        self.assertGreaterEqual(arrivals[3] - arrivals[2], 0.018)

    def test_denied_source_never_listens_or_calls_source(self):
        fmt, ep = spec(), endpoint()
        calls = []
        send = Session("origin", [], consent=True)
        with self.assertRaises(PermissionError):
            StreamSource(send, fmt, lambda: calls.append(1), check_lease=checked,
                         validate_block=validated, endpoint=ep)
        self.assertEqual(calls, [])
        with socket.socket() as retry:
            retry.bind((ep.host, ep.port))
        with self.assertRaises(PermissionError):
            StreamSource(sessions(fmt, ep)[0], fmt, lambda: calls.append(1),
                         check_lease=lambda: (_ for _ in ()).throw(PermissionError()),
                         validate_block=validated, endpoint=ep)
        self.assertEqual(calls, [])

    def test_failed_worker_start_releases_endpoint(self):
        fmt, ep = spec(), endpoint()
        with patch.object(threading.Thread, "start", side_effect=RuntimeError("worker start")):
            with self.assertRaisesRegex(RuntimeError, "worker start"):
                StreamSource(sessions(fmt, ep)[0], fmt, lambda: None, check_lease=checked,
                             validate_block=validated, endpoint=ep)
        with socket.socket() as retry:
            retry.bind((ep.host, ep.port))

    def test_media_permission_cannot_activate_live_receive(self):
        fmt, ep = spec(), endpoint()
        receive = Session("sink", [Grant("media-fetch", ep, "origin", fmt)], consent=True)
        called = []
        with self.assertRaises(PermissionError):
            StreamReceiver(receive).receive(Resource(ep, "/stream", fmt), called.append,
                                            check_lease=checked, accept_block=admit, block_factory=Block)
        self.assertEqual(called, [])

    def test_source_revoke_during_cadence_stops_without_another_read(self):
        fmt, ep = spec(rate=20), endpoint()  # One second per source block.
        send, receive = sessions(fmt, ep)
        calls = []
        received = []

        def read():
            calls.append(1)
            return block(fmt)

        def consume(value):
            received.append(value)
            send.revoke()

        with StreamSource(send, fmt, read, check_lease=checked,
                          validate_block=validated, endpoint=ep) as source:
            with self.assertRaises((ProtocolError, OSError)):
                StreamReceiver(receive).receive(source.resource, consume,
                                                check_lease=checked, accept_block=admit, block_factory=Block)
        self.assertEqual(len(received), 1)
        self.assertEqual(calls, [1])

    def test_disconnect_during_source_cadence_stops_reading_and_releases_active_worker(self):
        fmt, ep = spec(rate=20), endpoint()
        finished = threading.Event()
        calls = []

        class ObservedSource(StreamSource):
            def _serve(self, sock):
                try:
                    return super()._serve(sock)
                finally:
                    finished.set()

        with ObservedSource(sessions(fmt, ep)[0], fmt, lambda: calls.append(1) or block(fmt),
                            check_lease=checked, validate_block=validated, endpoint=ep) as source:
            conn = http.client.HTTPConnection(ep.host, ep.port, timeout=2)
            conn.request("GET", "/stream")
            response = conn.getresponse()
            self.assertTrue(response.read(1))
            response.close()
            conn.close()
            self.assertTrue(finished.wait(0.5))
            self.assertEqual(source.errors, 1)
        self.assertEqual(calls, [1])

    def test_close_interrupts_long_cadence_and_joins_owned_worker(self):
        fmt, ep = spec(rate=1), endpoint()  # Period > request timeout.
        source = StreamSource(sessions(fmt, ep)[0], fmt, lambda: block(fmt),
                              check_lease=checked, validate_block=validated, endpoint=ep)
        conn = http.client.HTTPConnection(ep.host, ep.port, timeout=2)
        try:
            conn.request("GET", "/stream")
            response = conn.getresponse()
            response.read(1)
            started = time.monotonic()
            source.close()
            self.assertLess(time.monotonic() - started, 0.5)
            self.assertFalse(source._thread.is_alive())
            self.assertEqual(source._sock.fileno(), -1)
            response.close()
        finally:
            conn.close()
            source.close()

    def test_pending_block_revalidated_after_source_callback(self):
        fmt, ep = spec(), endpoint()
        received = []

        def reject(value):
            raise PermissionError("stale generation")

        with StreamSource(sessions(fmt, ep)[0], fmt, lambda: block(fmt), check_lease=checked,
                          validate_block=reject, endpoint=ep) as source:
            with self.assertRaises(ProtocolError):
                self.receive(fmt, ep, received.append)
        self.assertEqual(received, [])
        self.assertEqual(source.last_error, "PermissionError")

    def test_source_rejects_wrong_spec_partial_frames_and_packet_overflow(self):
        fmt = spec()
        for bad in (replace(block(fmt), spec=replace(fmt, channels=("X",))),
                    replace(block(fmt), pcm=b"x"),
                    replace(block(fmt), pcm=bytes(fmt.frame_bytes * 21))):
            with self.subTest(block=bad):
                ep = endpoint()
                delivered = []
                with StreamSource(sessions(fmt, ep)[0], fmt, lambda: bad, check_lease=checked,
                                  validate_block=validated, endpoint=ep):
                    with self.assertRaises(ProtocolError):
                        self.receive(fmt, ep, delivered.append)
                self.assertEqual(delivered, [])

    def test_hostile_response_framing_and_metadata_never_reach_consumer(self):
        fmt = spec()
        cases = {
            "redirect": response_head(fmt, first=b"HTTP/1.1 302 Found"),
            "wrong_spec": response_head(replace(fmt, sample_rate=2000)),
            "wrong_profile": response_head(fmt, profile="other"),
            "ambiguous_length": response_head(fmt, extra=b"Content-Length: 0\r\n"),
            "encoded": response_head(fmt, extra=b"Content-Encoding: gzip\r\n"),
            "duplicate_header": response_head(fmt, extra=b"Transfer-Encoding: chunked\r\n"),
            "oversized_record": response_head(fmt) + f"{MAX_RECORD + 1:x}\r\n".encode(),
            "size_line_budget": response_head(fmt) + b"0" * 11,
            "extension": response_head(fmt) + b"A;foo=1\r\n",
            "trailer": response_head(fmt) + b"0\r\nX: y\r\n\r\n",
            "truncated": response_head(fmt) + b"100\r\nabc",
            "missing_metadata": response_head(fmt) + b"1\r\nx\r\n",
            "duplicate_json": response_head(fmt) + wire_record(fmt, meta=b'{"generation":3,"generation":3}'),
            "metadata_depth": response_head(fmt) + wire_record(fmt, meta=b"[" * 400 + b"]" * 400),
            "metadata_size": response_head(fmt) + wire_record(fmt, meta=b"x" * 1025),
            "missing_field": response_head(fmt) + wire_record(fmt, meta={"generation": 3}),
            "unknown_field": response_head(fmt) + wire_record(fmt, meta=metadata(secret=True)),
            "bool_generation": response_head(fmt) + wire_record(fmt, meta=metadata(generation=True)),
            "float_revision": response_head(fmt) + wire_record(fmt, meta=metadata(revision=5.0)),
            "empty_epoch": response_head(fmt) + wire_record(fmt, meta=metadata(source_epoch="")),
            "zero_clock": response_head(fmt) + wire_record(fmt, meta=metadata(clock_id=0)),
            "frame_overflow": response_head(fmt) + wire_record(fmt, meta=metadata(base_frame=2**64 - 1)),
            "partial_frame": response_head(fmt) + wire_record(fmt, pcm=b"x"),
            "packet_frames": response_head(fmt) + wire_record(fmt, pcm=bytes(fmt.frame_bytes * 21)),
        }
        for name, response in cases.items():
            with self.subTest(case=name), peer(response) as ep:
                delivered = []
                with self.assertRaises(ProtocolError):
                    self.receive(fmt, ep, delivered.append)
                self.assertEqual(delivered, [])

    def test_cancellation_and_session_revoke_interrupt_stalled_record(self):
        fmt = spec()
        for action in ("cancel", "revoke"):
            sent, release = threading.Event(), threading.Event()

            def stall(conn):
                sent.set()
                release.wait(1)

            with self.subTest(action=action), peer(response_head(fmt) + b"100\r\nx", after_send=stall) as ep:
                send, receive = sessions(fmt, ep)
                cancel = threading.Event()
                failures, delivered = [], []

                def run():
                    try:
                        StreamReceiver(receive).receive(Resource(ep, "/stream", fmt), delivered.append,
                                                        check_lease=checked, accept_block=admit, block_factory=Block, cancel=cancel)
                    except Exception as exc:
                        failures.append(exc)

                worker = threading.Thread(target=run)
                worker.start()
                try:
                    self.assertTrue(sent.wait(1))
                    cancel.set() if action == "cancel" else receive.revoke()
                    worker.join(0.5)
                    self.assertFalse(worker.is_alive())
                    self.assertEqual(len(failures), 1)
                    self.assertIsInstance(failures[0], PermissionError)
                    self.assertEqual(delivered, [])
                finally:
                    release.set()
                    worker.join(2)

    def test_idle_peer_deadline_applies_to_partial_record_without_whole_stream_deadline(self):
        fmt = spec()
        release = threading.Event()
        with peer(response_head(fmt) + b"100\r\nx", after_send=lambda c: release.wait(1)) as ep:
            try:
                with patch("mixyard_interop.http_upnp.stream.IO_TIMEOUT", 0.15):
                    with self.assertRaises(TimeoutError):
                        self.receive(fmt, ep, lambda b: self.fail("partial record released"))
            finally:
                release.set()

    def test_continuous_records_outlive_one_io_deadline(self):
        fmt, ep = spec(), endpoint()
        send, receive = sessions(fmt, ep)
        produced = 0

        def read():
            nonlocal produced
            if produced == 16:
                return None
            value = block(fmt, produced * 20)
            produced += 1
            return value

        with patch("mixyard_interop.http_upnp.stream.IO_TIMEOUT", 0.15):
            with StreamSource(send, fmt, read, check_lease=checked,
                              validate_block=validated, endpoint=ep) as source:
                started = time.monotonic()
                result = StreamReceiver(receive).receive(source.resource, lambda b: None,
                                                        check_lease=checked, accept_block=admit, block_factory=Block)
                self.assertGreater(time.monotonic() - started, 0.3)
                self.assertEqual(result.blocks, 16)

    def test_slow_source_callback_result_is_not_published_after_deadline(self):
        fmt, ep = spec(), endpoint()

        def slow():
            threading.Event().wait(0.2)
            return block(fmt)

        with patch("mixyard_interop.http_upnp.stream.IO_TIMEOUT", 0.15):
            with StreamSource(sessions(fmt, ep)[0], fmt, slow, check_lease=checked,
                              validate_block=validated, endpoint=ep) as source:
                # A separate longer client deadline observes the source timeout
                # rather than closing/cancelling it first at the same instant.
                conn = http.client.HTTPConnection(ep.host, ep.port, timeout=2)
                try:
                    conn.request("GET", "/stream")
                    response = conn.getresponse()
                    with self.assertRaises(http.client.IncompleteRead) as failure:
                        response.read()
                    self.assertEqual(failure.exception.partial, b"")
                    response.close()
                finally:
                    conn.close()
            self.assertEqual(source.last_error, "TimeoutError")


if __name__ == "__main__":
    unittest.main()
