"""Executable synthetic loopback proofs; no capture, playback or third-party SDK."""

import json
import socket
import struct
import threading
import unittest
from unittest.mock import patch

from mixyard_interop.core import Endpoint, Grant, Session, Spec
from mixyard_interop.realtime import RtpProfile, RtpReceiver, RtpSender, PacketRejected
from mixyard_interop.realtime import RtspClient, SnapcastControl
from mixyard_interop.realtime._tcp import ControlError


def spec(codec="raw-s16-le", channels=("FL", "FR"), rate=44100, frames=2):
    return Spec.from_dict({
        "stream": 1, "name": "Synthetic bus", "codec": codec, "sample_rate": rate,
        "channels": list(channels), "frames_per_packet": frames,
        "provenance": {"sources": [{"peer": "origin", "source": "file", "groups": ["synthetic"]}],
                       "visited_peers": ["origin"]},
    })


def session(role, endpoint, media=None, *, local="origin", peer="destination"):
    return Session(local, (Grant(role, endpoint, peer, media),), consent=True)


def udp_socket():
    result = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    result.bind(("127.0.0.1", 0))
    result.settimeout(0.2)
    return result


def udp_endpoint(sock):
    return Endpoint("127.0.0.1", sock.getsockname()[1], "udp")


def unused_udp_endpoint():
    with udp_socket() as sock:
        return udp_endpoint(sock)


class TcpPeer:
    """One owned bounded synthetic wire peer; failures propagate to the test."""

    def __init__(self, handler, scheme="tcp"):
        self.listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen(1)
        self.listener.settimeout(1)
        self.endpoint = Endpoint("127.0.0.1", self.listener.getsockname()[1], scheme)
        self.handler = handler
        self.errors = []
        self.thread = threading.Thread(target=self._run, daemon=True)

    def _run(self):
        try:
            connection, _address = self.listener.accept()
            with connection:
                connection.settimeout(0.8)
                self.handler(connection)
        except BaseException as exc:
            self.errors.append(exc)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_exc):
        self.listener.close()
        self.thread.join(1.2)
        if self.thread.is_alive():
            raise AssertionError("owned TCP peer did not stop")
        if self.errors:
            raise self.errors[0]


def read_until(connection, delimiter=b"\n"):
    data = bytearray()
    for _ in range(8192):
        part = connection.recv(1)
        if not part:
            raise AssertionError("client disconnected before request")
        data.extend(part)
        if data.endswith(delimiter):
            return bytes(data)
    raise AssertionError("test request budget exceeded")


class RtpTests(unittest.TestCase):
    def test_l16_golden_wire_and_l24_golden_wire(self):
        vectors = [
            ("raw-s16-le", bytes.fromhex("0000ff7f0080ffff"), bytes.fromhex("00007fff8000ffff")),
            ("raw-s24-le", bytes.fromhex("000000ffff7f000080ffffff"), bytes.fromhex("0000007fffff800000ffffff")),
        ]
        for codec, local, wire in vectors:
            with self.subTest(codec=codec), udp_socket() as peer:
                endpoint = udp_endpoint(peer)
                profile = RtpProfile(spec(codec))
                grant = session("live-send", endpoint, profile.spec)
                with RtpSender(grant, endpoint, profile, ssrc=0x01020304, sequence=65535,
                               timestamp=0xFFFFFFFE) as sender:
                    sender.send(local)
                    packet, address = peer.recvfrom(1201)
                    self.assertEqual(packet, bytes.fromhex("8060ffffff fffffe01020304".replace(" ", "")) + wire)
                    self.assertEqual(address, (sender.local.host, sender.local.port))
                    sender.send(local)
                    packet, _ = peer.recvfrom(1201)
                    self.assertEqual(struct.unpack("!HII", packet[2:12]), (0, 0, 0x01020304))

    def test_both_roles_preserve_exact_multichannel_spec_and_pcm(self):
        for codec in ("raw-s16-le", "raw-s24-le"):
            with self.subTest(codec=codec):
                media = spec(codec, ("FR", "FL", "C"), rate=96000, frames=3)
                profile = RtpProfile(media, 101)
                destination = unused_udp_endpoint()
                with RtpSender(session("live-send", destination, media), destination, profile, ssrc=7) as sender:
                    receive_grant = session("live-receive", sender.local, media, local="destination", peer="origin")
                    with RtpReceiver(receive_grant, destination, sender.local, profile, ssrc=7) as receiver:
                        pcm = bytes(range(profile.payload_bytes))
                        sender.send(pcm)
                        received = receiver.receive()
                        self.assertEqual(received.pcm, pcm)
                        self.assertIs(received.spec, media)
                        self.assertEqual(received.spec.channels, ("FR", "FL", "C"))
                        self.assertEqual(received.missing_before, 0)

    def test_rollover_loss_duplicate_reorder_timestamp_and_ssrc_validation(self):
        media = spec(frames=1)
        profile = RtpProfile(media)
        with udp_socket() as peer:
            endpoint = udp_endpoint(peer)
            local = unused_udp_endpoint()
            grant = session("live-receive", endpoint, media, local="destination", peer="origin")
            with RtpReceiver(grant, local, endpoint, profile, ssrc=9) as receiver:
                def emit(seq, stamp, *, ssrc=9, flags=0x80, pt=96, payload=b"\x00\x01\x00\x02"):
                    peer.sendto(struct.pack("!BBHII", flags, pt, seq, stamp, ssrc) + payload,
                                (local.host, local.port))
                emit(65534, 0xFFFFFFFE)
                self.assertEqual(receiver.receive().sequence, 65534)
                emit(0, 0)
                received = receiver.receive()
                self.assertEqual(received.missing_before, 1)
                self.assertEqual(received.pcm, b"\x01\x00\x02\x00")
                for seq, stamp, kwargs in [
                    (0, 0, {}), (65535, 0xFFFFFFFF, {}), (129, 129, {}),
                    (1, 8, {}), (1, 1, {"ssrc": 8}), (1, 1, {"flags": 0x90}),
                    (1, 1, {"pt": 97}), (1, 1, {"payload": b"\0" * 1201}),
                ]:
                    emit(seq, stamp, **kwargs)
                    with self.assertRaises(PacketRejected):
                        receiver.receive()
                emit(1, 1)
                self.assertEqual(receiver.receive().sequence, 1)
                self.assertEqual((receiver.received, receiver.missing, receiver.rejected), (3, 1, 8))

    def test_unadmitted_source_cannot_establish_sequence_or_deliver(self):
        media = spec(frames=1)
        with udp_socket() as good, udp_socket() as attacker:
            endpoint, local = udp_endpoint(good), unused_udp_endpoint()
            grant = session("live-receive", endpoint, media, local="destination", peer="origin")
            with RtpReceiver(grant, local, endpoint, RtpProfile(media), ssrc=3) as receiver:
                packet = bytes.fromhex("80600001000000010000000300010002")
                attacker.sendto(packet, (local.host, local.port))
                with self.assertRaisesRegex(PacketRejected, "unadmitted"):
                    receiver.receive()
                good.sendto(packet, (local.host, local.port))
                self.assertEqual(receiver.receive().sequence, 1)

    def test_revocation_and_exact_role_format_endpoint_denials(self):
        media = spec()
        with udp_socket() as peer:
            endpoint = udp_endpoint(peer)
            for grant in [Session("origin", consent=True), session("control", endpoint),
                          session("live-send", endpoint, spec(rate=48000))]:
                with self.assertRaises(PermissionError):
                    RtpSender(grant, endpoint, RtpProfile(media))
            grant = session("live-send", endpoint, media)
            with RtpSender(grant, endpoint, RtpProfile(media)) as sender:
                grant.revoke()
                with self.assertRaises(PermissionError):
                    sender.send(bytes(8))
                with self.assertRaises(socket.timeout):
                    peer.recvfrom(1201)

    def test_queued_pcm_cannot_escape_after_revocation_during_receive(self):
        media = spec(frames=1)
        with udp_socket() as peer:
            endpoint, local = udp_endpoint(peer), unused_udp_endpoint()
            grant = session("live-receive", endpoint, media, local="destination", peer="origin")
            with RtpReceiver(grant, local, endpoint, RtpProfile(media), ssrc=1) as receiver:
                started = threading.Event()
                errors = []
                original = grant.require
                def admitted(role, endpoint, spec=None):
                    original(role, endpoint, spec)
                    started.set()
                def receive():
                    try:
                        receiver.receive()
                    except BaseException as exc:
                        errors.append(exc)
                with patch.object(grant, "require", side_effect=admitted):
                    thread = threading.Thread(target=receive, daemon=True)
                    thread.start()
                    self.assertTrue(started.wait(0.2))
                    grant.revoke()
                    peer.sendto(bytes.fromhex("80600001000000010000000100000000"), (local.host, local.port))
                    thread.join(0.5)
                    self.assertFalse(thread.is_alive())
                self.assertEqual(len(errors), 1)
                self.assertIsInstance(errors[0], PermissionError)
                self.assertEqual(receiver.received, 0)

    def test_format_and_work_bounds_reject_without_implicit_conversion(self):
        for media in (spec("raw-f32-le"), spec(frames=300)):
            with self.assertRaises(ValueError):
                RtpProfile(media)
        media = spec()
        profile = RtpProfile(media)
        with self.assertRaises(ValueError):
            profile.convert_endian(b"\0")
        with udp_socket() as peer:
            endpoint = udp_endpoint(peer)
            with RtpSender(session("live-send", endpoint, media), endpoint, profile, max_packets=1) as sender:
                sender.send(bytes(8))
                peer.recvfrom(1201)
                with self.assertRaisesRegex(RuntimeError, "budget"):
                    sender.send(bytes(8))
        self.assertIn(b"a=rtpmap:96 L16/44100/2\r\n", profile.sdp(Endpoint("127.0.0.1", 5004, "udp")))
        with self.assertRaises(ValueError):
            RtpProfile(spec(channels=("FR", "FL"))).sdp(Endpoint("127.0.0.1", 5004, "udp"))

    def test_receiver_requires_explicit_local_endpoint_and_bounds_absent_peer(self):
        media = spec()
        with udp_socket() as peer:
            endpoint = udp_endpoint(peer)
            grant = session("live-receive", endpoint, media, local="destination", peer="origin")
            with self.assertRaises(ValueError):
                RtpReceiver(grant, None, endpoint, RtpProfile(media), ssrc=1)
            with RtpReceiver(grant, unused_udp_endpoint(), endpoint, RtpProfile(media),
                             ssrc=1, timeout=0.01, max_packets=1) as receiver:
                with self.assertRaises(socket.timeout):
                    receiver.receive()
                with self.assertRaisesRegex(RuntimeError, "budget"):
                    receiver.receive()

    def test_bridge_exclusion_loop_and_expired_consent_fail_before_io(self):
        media = spec()
        endpoint = Endpoint("127.0.0.1", 5004, "udp")
        grant = Grant("live-send", endpoint, "destination", media)
        with self.assertRaises(PermissionError):
            Session("origin", (grant,), consent=True, excluded_groups=("synthetic",))
        loop = media.to_dict()
        loop["provenance"]["visited_peers"] = ["origin", "destination"]
        loop_media = Spec.from_dict(loop)
        with self.assertRaises(ValueError):
            Session("destination", (Grant("live-send", endpoint, "origin", loop_media),), consent=True)
        consent = session("live-send", endpoint, media)
        with patch("mixyard_interop.core.time.monotonic", return_value=float("inf")):
            with patch("mixyard_interop.realtime.rtp.socket.socket") as opened:
                with self.assertRaises(PermissionError):
                    RtpSender(consent, endpoint, RtpProfile(media))
                opened.assert_not_called()


class RtspTests(unittest.TestCase):
    def test_public_control_lifecycle_uses_pinned_url_and_never_grants_media(self):
        requests = []
        sdp = b"v=0\r\ns=Untrusted metadata\r\n"
        def handler(conn):
            for index, method in enumerate(("OPTIONS", "DESCRIBE", "SETUP", "PLAY", "PAUSE", "TEARDOWN"), 1):
                request = read_until(conn, b"\r\n\r\n")
                requests.append(request)
                self.assertTrue(request.startswith(f"{method} rtsp://127.0.0.1:".encode()))
                self.assertIn(f"CSeq: {index}\r\n".encode(), request)
                headers = [f"RTSP/1.0 200 OK\r\nCSeq: {index}\r\n"]
                body = b""
                if method == "DESCRIBE":
                    headers.append("Content-Type: application/sdp\r\n")
                    body = sdp
                if method == "SETUP":
                    headers.append("Session: abc123;timeout=60\r\nTransport: RTP/AVP;unicast;client_port=5004-5005;server_port=6000-6001;ssrc=12345678\r\n")
                    self.assertIn(b"/stream/track1 RTSP/1.0", request)
                if method in ("PLAY", "PAUSE", "TEARDOWN"):
                    self.assertIn(b"Session: abc123\r\n", request)
                    headers.append("Session: abc123\r\n")
                conn.sendall(("".join(headers) + f"Content-Length: {len(body)}\r\n\r\n").encode() + body)
        with TcpPeer(handler, "rtsp") as peer:
            grant = session("control", peer.endpoint)
            with RtspClient(grant, peer.endpoint, "/stream") as client:
                client.options()
                self.assertEqual(client.describe().body, sdp)
                client.setup(Endpoint("127.0.0.1", 5004, "udp"), Endpoint("127.0.0.1", 5005, "udp"), track_path="/stream/track1")
                self.assertEqual(client.server_ports, (6000, 6001))
                client.play()
                self.assertEqual(client.state, "playing")
                client.pause()
                client.teardown()
                self.assertEqual(client.state, "init")
                with self.assertRaises(PermissionError):
                    RtpSender(grant, Endpoint("127.0.0.1", 6000, "udp"), RtpProfile(spec()))
        self.assertEqual(len(requests), 6)

    def test_malformed_redirect_duplicate_and_length_responses_fail_closed(self):
        responses = [
            b"RTSP/1.0 302 Found\r\nCSeq: 1\r\nLocation: rtsp://example.com/private\r\n\r\n",
            b"RTSP/1.0 200 OK\r\nCSeq: 2\r\n\r\n",
            b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nCSeq: 1\r\n\r\n",
            b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Length: 99999\r\n\r\n",
            b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nX: " + b"x" * 8192 + b"\r\n\r\n",
            b"RTSP/2.0 200 OK\r\nCSeq: 1\r\n\r\n",
        ]
        for response in responses:
            def handler(conn):
                read_until(conn, b"\r\n\r\n")
                conn.sendall(response)
            with self.subTest(response=response[:60]), TcpPeer(handler, "rtsp") as peer:
                with RtspClient(session("control", peer.endpoint), peer.endpoint, "/stream") as client:
                    with self.assertRaises(ControlError):
                        client.options()
                    self.assertIsNone(client._socket)

    def test_transport_cannot_retarget_media_or_switch_to_multicast(self):
        for transport in ("RTP/AVP;multicast;client_port=5004-5005;server_port=6000-6001",
                          "RTP/AVP;unicast;client_port=5004-5005;server_port=6000-6001;source=192.0.2.1",
                          "RTP/AVP;unicast;client_port=5010-5011;server_port=6000-6001",
                          "RTP/AVP/TCP;unicast;interleaved=0-1"):
            def handler(conn):
                read_until(conn, b"\r\n\r\n")
                conn.sendall(f"RTSP/1.0 200 OK\r\nCSeq: 1\r\nSession: abc\r\nTransport: {transport}\r\n\r\n".encode())
            with self.subTest(transport=transport), TcpPeer(handler, "rtsp") as peer:
                with RtspClient(session("control", peer.endpoint), peer.endpoint, "/stream") as client:
                    with self.assertRaises(ControlError):
                        client.setup(Endpoint("127.0.0.1", 5004, "udp"), Endpoint("127.0.0.1", 5005, "udp"))
                    self.assertEqual(client.state, "init")

    def test_control_revocation_blocks_next_request(self):
        def handler(conn):
            read_until(conn, b"\r\n\r\n")
            conn.sendall(b"RTSP/1.0 200 OK\r\nCSeq: 1\r\n\r\n")
            self.assertEqual(conn.recv(1), b"")
        with TcpPeer(handler, "rtsp") as peer:
            grant = session("control", peer.endpoint)
            with RtspClient(grant, peer.endpoint, "/stream") as client:
                client.options()
                grant.revoke()
                with self.assertRaises(PermissionError):
                    client.options()


class SnapcastTests(unittest.TestCase):
    def test_silent_control_peer_is_bounded_and_closed_without_retry(self):
        def handler(conn):
            read_until(conn)
            self.assertEqual(conn.recv(1), b"")
        with TcpPeer(handler) as peer:
            with SnapcastControl(session("service-control", peer.endpoint), peer.endpoint,
                                 timeout=0.02) as client:
                with self.assertRaises(TimeoutError):
                    client.get_status()
                self.assertIsNone(client._socket)
                self.assertEqual(client._requests, 1)

    def test_public_ndjson_status_volume_and_mute_with_notifications(self):
        expected = [
            ("Server.GetRPCVersion", None, {"major": 2, "minor": 0, "patch": 0}),
            ("Server.GetStatus", None, {"server": {"groups": [], "streams": []}}),
            ("Client.GetStatus", {"id": "test-client"}, {"client": {"id": "test-client"}}),
            ("Client.SetVolume", {"id": "test-client", "volume": {"muted": True, "percent": 74}}, {"volume": {"muted": True, "percent": 74}}),
            ("Group.SetMute", {"id": "group-1", "mute": True}, {"mute": True}),
        ]
        def handler(conn):
            for index, (method, params, result) in enumerate(expected, 1):
                request = json.loads(read_until(conn))
                self.assertEqual(request, {"jsonrpc": "2.0", "id": index, "method": method,
                                          **({"params": params} if params is not None else {})})
                notification = {"jsonrpc": "2.0", "method": "Client.OnConnect", "params": {}}
                response = {"jsonrpc": "2.0", "id": index, "result": result}
                # TCP segmentation and multiple newline messages in one read.
                data = (json.dumps(notification) + "\n" + json.dumps(response) + "\n").encode()
                conn.sendall(data[:7])
                conn.sendall(data[7:])
        with TcpPeer(handler) as peer:
            with SnapcastControl(session("service-control", peer.endpoint), peer.endpoint) as client:
                self.assertEqual(client.get_rpc_version(), expected[0][2])
                self.assertEqual(client.get_status(), expected[1][2])
                self.assertEqual(client.get_client_status("test-client"), expected[2][2])
                self.assertEqual(client.set_client_volume("test-client", percent=74, muted=True), expected[3][2])
                self.assertEqual(client.set_group_mute("group-1", muted=True), expected[4][2])

    def test_bad_json_response_correlation_depth_bytes_and_notification_budget(self):
        responses = [
            b'{"jsonrpc":"2.0","id":2,"result":{}}\n',
            b'{"jsonrpc":"2.0","id":1,"id":1,"result":{}}\n',
            b'{"jsonrpc":"2.0","id":1,"error":{"code":-1}}\n',
            b'{"jsonrpc":"2.0","id":1,"result":1e999}\n',
            b'{"jsonrpc":"2.0","id":1,"result":' + b"[" * 17 + b"0" + b"]" * 17 + b"}\n",
            b"x" * 16384 + b"\n",
            b'{"jsonrpc":"2.0","method":"Client.OnConnect"}\n' * 9,
        ]
        for response in responses:
            def handler(conn):
                read_until(conn)
                conn.sendall(response)
            with self.subTest(response=response[:70]), TcpPeer(handler) as peer:
                with SnapcastControl(session("service-control", peer.endpoint), peer.endpoint) as client:
                    with self.assertRaises(ControlError):
                        client.get_status()
                    self.assertIsNone(client._socket)

    def test_control_permission_does_not_imply_service_control_or_audio(self):
        endpoint = Endpoint("127.0.0.1", 1705, "tcp")
        with SnapcastControl(session("control", endpoint), endpoint) as client:
            with self.assertRaises(PermissionError):
                client.get_status()
        with SnapcastControl(Session("origin", consent=True), endpoint) as client:
            with self.assertRaises(PermissionError):
                client.set_group_mute("group", muted=False)

    def test_revocation_while_response_waits_blocks_release(self):
        grant = None
        def handler(conn):
            read_until(conn)
            grant.revoke()
            conn.sendall(b'{"jsonrpc":"2.0","id":1,"result":{}}\n')
        with TcpPeer(handler) as peer:
            grant = session("service-control", peer.endpoint)
            with SnapcastControl(grant, peer.endpoint) as client:
                with self.assertRaises(PermissionError):
                    client.get_status()


if __name__ == "__main__":
    unittest.main()
