"""Real owned-loopback IO with production authorization; no audio devices."""
import http.client
import socket
import threading
import time
import unittest
from dataclasses import replace
from unittest.mock import patch

from mixyard_interop.core import Endpoint, Grant, Session, Spec
from mixyard_interop.http_upnp import (Advertiser, Controller, HttpReceiver, MediaSource,
                                      NoticeReceiver, ProtocolError, Renderer, Resource, discover)
from mixyard_interop.http_upnp.http import MAX_HEADER, MAX_MEDIA, Server, request, url
from mixyard_interop.http_upnp.ssdp import _parse
from mixyard_interop.http_upnp.upnp import AVT, envelope, metadata, xml


def endpoint(scheme="http"):
    kind = socket.SOCK_DGRAM if scheme == "udp" else socket.SOCK_STREAM
    with socket.socket(socket.AF_INET, kind) as sock:
        sock.bind(("127.0.0.1", 0))
        return Endpoint("127.0.0.1", sock.getsockname()[1], scheme)


def spec(codec="raw-s24-le", channels=("FR", "FL", "AUX"), rate=96000, groups=()):
    return Spec.from_dict({"stream": 37, "name": "selected mix", "codec": codec,
                          "sample_rate": rate, "channels": list(channels), "frames_per_packet": 2,
                          "provenance": {"sources": [{"peer": "origin", "source": "file-7", "groups": list(groups)}],
                                         "visited_peers": ["origin"]}})


def session(local, grants, **kwargs):
    return Session(local, grants, consent=True, **kwargs)


class HTTPTests(unittest.TestCase):
    def test_exact_raw_profiles_and_live_media_role_separation(self):
        for codec in ("raw-s16-le", "raw-s24-le", "raw-s32-le", "raw-f32-le", "raw-f64-le"):
            for mode in ("media", "live"):
                with self.subTest(codec=codec, mode=mode):
                    fmt, ep = spec(codec), endpoint()
                    pcm = bytes((i * 53 + 1) % 256 for i in range(fmt.frame_bytes * 5))
                    send = "media-serve" if mode == "media" else "live-send"
                    receive = "media-fetch" if mode == "media" else "live-receive"
                    source_session = session("origin", [Grant(send, ep, "sink", fmt)])
                    sink_session = session("sink", [Grant(receive, ep, "origin", fmt)])
                    with MediaSource(source_session, fmt, pcm, endpoint=ep, mode=mode) as source:
                        self.assertEqual(HttpReceiver(sink_session).fetch(source.resource, mode=mode), pcm)
                        self.assertEqual(source.resource.spec.to_dict(), fmt.to_dict())
                        wrong_mode = "live" if mode == "media" else "media"
                        with self.assertRaises(PermissionError):
                            HttpReceiver(sink_session).fetch(source.resource, mode=wrong_mode)

    def test_l16_conversion_is_explicit_local_and_reversible(self):
        fmt, ep = spec("raw-s16-le", ("FL", "FR"), 44100), endpoint()
        pcm = b"\x34\x12\x00\x80\xff\x7f\x01\x00"
        send = session("origin", [Grant("media-serve", ep, "sink", fmt)])
        receive = session("sink", [Grant("media-fetch", ep, "origin", fmt)])
        with MediaSource(send, fmt, pcm, endpoint=ep, wire="l16") as source:
            headers, wire = request(receive, "media-fetch", ep, "/pcm", spec=fmt)
            self.assertEqual(wire, b"\x12\x34\x80\x00\x7f\xff\x00\x01")
            self.assertEqual(headers["content-type"], "audio/L16;rate=44100;channels=2")
            self.assertEqual(HttpReceiver(receive).fetch(source.resource), pcm)
        # Wider raw formats remain admitted; L16 does not silently narrow them.
        with self.assertRaises(ProtocolError):
            Resource(ep, "/pcm", spec("raw-f64-le"), "l16")

    def test_source_accepts_standard_http_get_without_content_length(self):
        fmt, ep = spec(), endpoint()
        pcm = bytes(range(fmt.frame_bytes))
        send = session("origin", [Grant("media-serve", ep, "sink", fmt)])
        receive = session("sink", [Grant("media-fetch", ep, "origin", fmt)])
        with MediaSource(send, fmt, pcm, endpoint=ep):
            receive.require("media-fetch", ep, fmt)
            conn = http.client.HTTPConnection(ep.host, ep.port, timeout=2)
            try:
                conn.request("GET", "/pcm")
                response = conn.getresponse()
                self.assertEqual(response.status, 200)
                self.assertEqual(response.read(), pcm)
            finally:
                conn.close()

    def test_preadmitted_generic_l16_source_needs_no_private_headers(self):
        fmt, ep = spec("raw-s16-le", ("FC",), 48000), endpoint()
        send = session("origin", [Grant("media-serve", ep, "sink", fmt)])
        receive = session("sink", [Grant("media-fetch", ep, "origin", fmt)])
        def generic_source(*args, **kwargs):
            return {"Content-Type": "audio/L16;rate=48000;channels=1"}, b"\x80\x00\x00\x01"
        with Server(send, "media-serve", generic_source, fmt, endpoint=ep):
            self.assertEqual(HttpReceiver(receive).fetch(Resource(ep, "/pcm", fmt, "l16")),
                             b"\x00\x80\x01\x00")

    def test_generic_l16_media_type_semantics_preserve_exact_admission(self):
        fmt = spec("raw-s16-le", ("FC",), 48000)
        accepted = ["audio/l16;rate=48000;channels=1",
                    "Audio/L16; CHANNELS=1; RATE=48000",
                    'audio/L16;rate="48000";channels="1"',
                    "audio/L16;rate=48000", r'audio/L16;rate="\48000"']
        rejected = ["audio/L16;channels=1", "audio/L16;rate=44100",
                    "audio/L16;rate=48000;channels=2", "audio/L16;rate=48000;rate=48000",
                    "audio/L16;rate=48000;CHANNELS=1;channels=1",
                    "audio/L16;rate=48000;unknown=1", 'audio/L16;rate="48000',
                    "audio/L16;rate=48000,channels=1", "audio/L16;rate=48000;",
                    "audio/L16;rate=48000.0", "audio/L16;rate=+48000",
                    'audio/L16;rate="48000;channels=2"']
        for mime in accepted + rejected:
            with self.subTest(content_type=mime):
                ep = endpoint()
                send = session("origin", [Grant("media-serve", ep, "sink", fmt)])
                receive = session("sink", [Grant("media-fetch", ep, "origin", fmt)])
                def generic_source(*args, **kwargs):
                    return {"Content-Type": mime}, b"\x80\x00\x00\x01"
                with Server(send, "media-serve", generic_source, fmt, endpoint=ep):
                    resource = Resource(ep, "/pcm", fmt, "l16")
                    if mime in accepted:
                        self.assertEqual(HttpReceiver(receive).fetch(resource), b"\x00\x80\x01\x00")
                    else:
                        with self.assertRaises(ProtocolError):
                            HttpReceiver(receive).fetch(resource)
        # An omitted channels parameter must never turn stereo admission into mono.
        stereo = spec("raw-s16-le", ("FL", "FR"), 48000)
        ep = endpoint()
        send = session("origin", [Grant("media-serve", ep, "sink", stereo)])
        receive = session("sink", [Grant("media-fetch", ep, "origin", stereo)])
        with Server(send, "media-serve", lambda *a, **kw: ({"Content-Type": "audio/L16;rate=48000"}, bytes(4)),
                    stereo, endpoint=ep):
            with self.assertRaises(ProtocolError):
                HttpReceiver(receive).fetch(Resource(ep, "/pcm", stereo, "l16"))

    def test_owned_listener_closes_during_partial_request(self):
        fmt, ep = spec(), endpoint()
        actual = session("origin", [Grant("media-serve", ep, "sink", fmt)])
        accepted = threading.Event()
        class ObservedSession:
            calls = 0
            def require(self, *args):
                actual.require(*args)
                self.calls += 1
                if self.calls >= 2:
                    accepted.set()
        source = MediaSource(ObservedSession(), fmt, bytes(fmt.frame_bytes), endpoint=ep)
        try:
            with socket.create_connection((ep.host, ep.port), timeout=2) as sock:
                sock.sendall(b"GET /pcm HTTP/1.1\r\n")
                self.assertTrue(accepted.wait(2))
                source.close()
                self.assertFalse(source._thread.is_alive())
                try:
                    self.assertEqual(sock.recv(1), b"")
                except ConnectionResetError:
                    pass  # Both EOF and reset close the incomplete request without media.
        finally:
            source.close()

    def test_receiver_and_sender_revocation_fail_closed(self):
        fmt, ep = spec(), endpoint()
        send = session("origin", [Grant("media-serve", ep, "sink", fmt)])
        receive = session("sink", [Grant("media-fetch", ep, "origin", fmt)])
        with MediaSource(send, fmt, bytes(fmt.frame_bytes * 4), endpoint=ep) as source:
            dispatch = source.dispatch
            def revoke_before_response(*args, **kwargs):
                result = dispatch(*args, **kwargs)
                receive.revoke()
                return result
            source.dispatch = revoke_before_response
            with self.assertRaises((PermissionError, OSError)):
                HttpReceiver(receive).fetch(source.resource)
            send.revoke()
            fresh = session("sink", [Grant("media-fetch", ep, "origin", fmt)])
            with self.assertRaises((ValueError, OSError)):
                HttpReceiver(fresh).fetch(source.resource)

    def test_exclusion_and_provenance_reject_whole_mix_before_listener(self):
        fmt, ep = spec(groups=("private-call",)), endpoint()
        with self.assertRaises(PermissionError):
            session("sink", [Grant("media-fetch", ep, "origin", fmt)], excluded_groups=("private-call",))
        with self.assertRaises(ValueError):
            session("origin", [Grant("media-fetch", ep, "origin", fmt)])
        with self.assertRaises(ValueError):
            session("sink", [Grant("media-fetch", ep, "impostor", fmt)])
        with self.assertRaises(PermissionError):
            Session("sink", consent=False)

    def test_exact_format_grant_and_wire_spec_cannot_be_relabelled(self):
        fmt, ep = spec(), endpoint()
        swapped = replace(fmt, channels=("FL", "FR", "AUX"))
        send = session("origin", [Grant("media-serve", ep, "sink", fmt)])
        receive = session("sink", [Grant("media-fetch", ep, "origin", swapped)])
        with MediaSource(send, fmt, bytes(fmt.frame_bytes), endpoint=ep) as source:
            with self.assertRaises(PermissionError):
                HttpReceiver(receive).fetch(source.resource)
            with self.assertRaises(ProtocolError):
                HttpReceiver(receive).fetch(Resource(ep, "/pcm", swapped))

    def test_budgets_and_malformed_header_recover_on_next_connection(self):
        fmt, ep = spec(), endpoint()
        send = session("origin", [Grant("media-serve", ep, "sink", fmt)])
        receive = session("sink", [Grant("media-fetch", ep, "origin", fmt)])
        with self.assertRaises(ProtocolError):
            MediaSource(send, fmt, b"x" * (MAX_MEDIA + 1), endpoint=ep)
        with self.assertRaises(ProtocolError):
            MediaSource(send, fmt, b"x", endpoint=ep)
        with MediaSource(send, fmt, bytes(fmt.frame_bytes), endpoint=ep) as source:
            for header in (b"Content-Length: 0\r\nContent-Length: 0\r\n",
                           b"Transfer-Encoding: chunked\r\nContent-Length: 0\r\n",
                           b"X-Fill: " + b"a" * MAX_HEADER + b"\r\nContent-Length: 0\r\n"):
                with socket.create_connection((ep.host, ep.port), timeout=2) as sock:
                    sock.sendall(f"GET /pcm HTTP/1.1\r\nHost: 127.0.0.1:{ep.port}\r\n".encode() + header + b"\r\n")
                    try:
                        self.assertEqual(sock.recv(1), b"")
                    except ConnectionResetError:
                        pass
                self.assertEqual(HttpReceiver(receive).fetch(source.resource), bytes(fmt.frame_bytes))
            self.assertEqual(source.errors, 3)

    def test_no_arbitrary_url_dns_or_scheme(self):
        fmt, ep = spec(), endpoint()
        for path in ("http://example.org/", "//example.org", "/../x", "/pcm?q=x", "/%2e%2e/x", "/x\r\nX: yes"):
            with self.subTest(path=path), self.assertRaises((ValueError, ProtocolError)):
                Resource(ep, path, fmt)
        for host in ("localhost", "127.1", "0.0.0.0", "::1", "169.254.169.254"):
            with self.assertRaises(ValueError):
                Endpoint(host, 80)
        with self.assertRaises(ProtocolError):
            Resource(Endpoint("127.0.0.1", 80, "tcp"), "/pcm", fmt)

    def test_redirect_oversized_truncated_responses_are_rejected(self):
        fmt = spec()
        for response in (b"HTTP/1.1 302 Found\r\nLocation: http://example.org/\r\nContent-Length: 0\r\n\r\n",
                         b"HTTP/1.1 200 OK\r\nContent-Length: 1048577\r\n\r\n",
                         b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nx"):
            with socket.socket() as server:
                server.bind(("127.0.0.1", 0))
                server.listen(1)
                server.settimeout(2)
                ep = Endpoint("127.0.0.1", server.getsockname()[1])
                receive = session("sink", [Grant("media-fetch", ep, "origin", fmt)])
                def respond():
                    conn, _ = server.accept()
                    with conn:
                        conn.settimeout(2)
                        data = b""
                        while not data.endswith(b"\r\n\r\n") and len(data) < MAX_HEADER:
                            part = conn.recv(1)
                            if not part:
                                return
                            data += part
                        conn.sendall(response)
                worker = threading.Thread(target=respond)
                worker.start()
                try:
                    with self.assertRaises(ProtocolError):
                        HttpReceiver(receive).fetch(Resource(ep, "/pcm", fmt))
                finally:
                    worker.join(3)
                    self.assertFalse(worker.is_alive())


class UPnPTests(unittest.TestCase):
    def test_failed_worker_start_releases_http_and_ssdp_endpoints(self):
        fmt, source_ep = spec(), endpoint()
        send = session("origin", [Grant("media-serve", source_ep, "sink", fmt)])
        failed = []
        def fail_start(worker):
            failed.append(worker._target.__self__)
            raise RuntimeError("injected worker-start failure")
        with patch.object(threading.Thread, "start", new=fail_start):
            with self.assertRaisesRegex(RuntimeError, "worker-start"):
                MediaSource(send, fmt, bytes(fmt.frame_bytes), endpoint=source_ep)
            self.assertEqual(failed[-1]._sock.fileno(), -1)
            with socket.socket() as retry:
                retry.bind((source_ep.host, source_ep.port))
            adv_ep, peer, renderer = endpoint("udp"), endpoint("udp"), endpoint()
            identity = "db9debd8-967b-49e7-a97c-26529fb5dc8d"
            adv = session("sink", [Grant("advertise", target, "searcher") for target in (adv_ep, peer, renderer)])
            with self.assertRaisesRegex(RuntimeError, "worker-start"):
                Advertiser(adv, renderer, identity, peer, endpoint=adv_ep)
            self.assertEqual(failed[-1]._sock.fileno(), -1)
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as retry:
                retry.bind((adv_ep.host, adv_ep.port))

    def test_withdrawal_failure_still_joins_worker_and_releases_udp_endpoint(self):
        adv_ep, peer, renderer = endpoint("udp"), endpoint("udp"), endpoint()
        identity = "db9debd8-967b-49e7-a97c-26529fb5dc8d"
        adv = session("sink", [Grant("advertise", target, "searcher") for target in (adv_ep, peer, renderer)])
        advertiser = Advertiser(adv, renderer, identity, peer, endpoint=adv_ep)
        try:
            advertiser.announce()
            with patch.object(advertiser, "announce", side_effect=OSError("injected withdrawal failure")):
                with self.assertRaisesRegex(OSError, "withdrawal"):
                    advertiser.close()
            self.assertFalse(advertiser._thread.is_alive())
            self.assertEqual(advertiser._sock.fileno(), -1)
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as retry:
                retry.bind((adv_ep.host, adv_ep.port))
        finally:
            advertiser.close()

    def test_play_fetch_shares_remaining_http_request_deadline(self):
        fmt, source_ep, render_ep = spec(), endpoint(), endpoint()
        send = session("origin", [Grant("media-serve", source_ep, "sink", fmt)])
        render = session("sink", [Grant("control", render_ep, "controller"),
                                  Grant("media-fetch", source_ep, "origin", fmt)])
        control = session("controller", [Grant("control", render_ep, "sink")])
        resource = Resource(source_ep, "/pcm", fmt)
        from mixyard_interop.http_upnp.http import _message
        def delayed_source(*args, **kwargs):
            threading.Event().wait(0.20)
            return {"Content-Type": "application/vnd.mixyard.pcm"}, bytes(fmt.frame_bytes)
        with patch("mixyard_interop.http_upnp.http.TIMEOUT", 0.32):
            with Server(send, "media-serve", delayed_source, fmt, endpoint=source_ep):
                with Renderer(render, {"mix-37": resource}, endpoint=render_ep) as renderer:
                    Controller(control, render_ep).set_uri("mix-37", resource)
                    body = envelope("Play", {"InstanceID": "0", "Speed": "1"})
                    message = _message("POST /control HTTP/1.1", {
                        "Host": f"127.0.0.1:{render_ep.port}", "Content-Type": "text/xml; charset=utf-8",
                        "SOAPAction": f'"{AVT}#Play"'}, body)
                    with socket.create_connection((render_ep.host, render_ep.port), timeout=2) as sock:
                        started = time.monotonic()
                        sock.sendall(message[:-1])
                        threading.Event().wait(0.20)
                        sock.sendall(message[-1:])
                        self.assertEqual(sock.recv(1), b"")
                        self.assertLess(time.monotonic() - started, 0.75)
                    self.assertEqual(renderer.take_pcm()[1], b"")
                    self.assertEqual(renderer.errors, 1)

    def test_controller_renderer_media_and_discovery_real_sockets(self):
        fmt, source_ep, render_ep = spec(), endpoint(), endpoint()
        pcm = bytes((i * 37) % 256 for i in range(fmt.frame_bytes * 9))
        send = session("origin", [Grant("media-serve", source_ep, "sink", fmt)])
        render = session("sink", [Grant("control", render_ep, "controller"), Grant("media-fetch", source_ep, "origin", fmt)])
        control = session("controller", [Grant("control", render_ep, "sink")])
        with MediaSource(send, fmt, pcm, endpoint=source_ep) as source:
            with Renderer(render, {"mix-37": source.resource}, endpoint=render_ep) as renderer:
                controller = Controller(control, render_ep)
                controller.set_uri("mix-37", source.resource)
                self.assertEqual(controller.info()["CurrentTransportState"], "STOPPED")
                controller.play()
                admitted, actual = renderer.take_pcm()
                self.assertEqual(actual, pcm)
                self.assertEqual(admitted.spec.provenance, fmt.provenance)
                self.assertEqual(controller.info()["CurrentTransportState"], "STOPPED")
                controller.play()
                controller.stop()
                self.assertEqual(renderer.take_pcm()[1], b"")
                for path in ("/device.xml", "/avtransport.xml"):
                    _, data = request(control, "control", render_ep, path)
                    self.assertIsNotNone(xml(data))
                adv_ep, search_ep = endpoint("udp"), endpoint("udp")
                adv = session("sink", [Grant("advertise", target, "searcher") for target in (adv_ep, search_ep, render_ep)])
                search = session("searcher", [Grant("advertise", target, "sink") for target in (adv_ep, search_ep)])
                with Advertiser(adv, render_ep, renderer.identity, search_ep, endpoint=adv_ep) as advertiser:
                    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as malformed:
                        malformed.bind((search_ep.host, search_ep.port))
                        malformed.sendto(b"M-SEARCH * HTTP/1.1\r\nST: bad\r\nST: duplicate\r\n\r\n", (adv_ep.host, adv_ep.port))
                    found = discover(search, adv_ep, local=search_ep, expected_renderer=render_ep, expected_identity=renderer.identity)
                    self.assertEqual(found.location, url(render_ep, "/device.xml"))
                    self.assertFalse(found.paired)
                    self.assertEqual(advertiser.errors, 1)
                with self.assertRaises(PermissionError):
                    Controller(search, render_ep).play()
                render.revoke()
                with self.assertRaises(PermissionError):
                    renderer.take_pcm()

    def test_control_alone_cannot_fetch_and_bad_soap_does_not_poison_renderer(self):
        fmt, source_ep, render_ep = spec(), endpoint(), endpoint()
        resource = Resource(source_ep, "/pcm", fmt)
        render = session("sink", [Grant("control", render_ep, "controller")])
        control = session("controller", [Grant("control", render_ep, "sink")])
        with Renderer(render, {"mix-37": resource}, endpoint=render_ep) as renderer:
            controller = Controller(control, render_ep)
            with self.assertRaises((ValueError, OSError)):
                controller.set_uri("mix-37", resource)
            self.assertEqual(controller.info()["CurrentTransportState"], "NO_MEDIA_PRESENT")
            for bad in (b'<!DOCTYPE a [<!ENTITY x "boom">]><a>&x;</a>',
                        envelope("Stop", {"InstanceID": "99"}),
                        b"<x>" * 9 + b"</x>" * 9,
                        envelope("Play", {"InstanceID": "0", "Speed": "1"})):
                with self.assertRaises((ValueError, OSError)):
                    request(control, "control", render_ep, "/control", method="POST", body=bad,
                            headers={"Content-Type": "text/xml; charset=utf-8", "SOAPAction": f'"{AVT}#Stop"'})
                self.assertEqual(controller.info()["CurrentTransportState"], "NO_MEDIA_PRESENT")
            self.assertEqual(renderer.errors, 5)

    def test_url_and_didl_source_identity_both_require_exact_approval(self):
        fmt, source_ep, render_ep = spec(), endpoint(), endpoint()
        resource = Resource(source_ep, "/pcm", fmt)
        render = session("sink", [Grant("control", render_ep, "controller"), Grant("media-fetch", source_ep, "origin", fmt)])
        control = session("controller", [Grant("control", render_ep, "sink")])
        with Renderer(render, {"mix-37": resource}, endpoint=render_ep) as renderer:
            controller = Controller(control, render_ep)
            for identity, uri in (("impostor", resource.uri), ("mix-37", "http://169.254.169.254/"),
                                  ("mix-37", resource.uri + "?other")):
                with self.assertRaises((ValueError, OSError)):
                    controller.action("SetAVTransportURI", InstanceID="0", CurrentURI=uri,
                                      CurrentURIMetaData=metadata(identity, resource))
                self.assertEqual(controller.info()["CurrentTransportState"], "NO_MEDIA_PRESENT")
            controller.set_uri("mix-37", resource)
            render.revoke()
            with self.assertRaises((ValueError, OSError)):
                controller.play()
            self.assertEqual(renderer._pcm, b"")

    def test_advertisement_announces_withdraws_and_revocation_stops_io(self):
        adv_ep, peer, renderer = endpoint("udp"), endpoint("udp"), endpoint()
        identity = "db9debd8-967b-49e7-a97c-26529fb5dc8d"
        adv = session("sink", [Grant("advertise", target, "searcher") for target in (adv_ep, peer, renderer)])
        search = session("searcher", [Grant("advertise", target, "sink") for target in (adv_ep, peer)])
        with NoticeReceiver(search, peer, adv_ep, renderer, identity) as notices:
            with Advertiser(adv, renderer, identity, peer, endpoint=adv_ep) as advertiser:
                advertiser.announce()
                notice = notices.receive()
                self.assertTrue(notice.alive)
                self.assertFalse(notice.paired)
            self.assertFalse(notices.receive().alive)
        adv = session("sink", [Grant("advertise", target, "searcher") for target in (adv_ep, peer, renderer)])
        with Advertiser(adv, renderer, identity, peer, endpoint=adv_ep) as advertiser:
            adv.revoke()
            with self.assertRaises(PermissionError):
                advertiser.announce()
        search.revoke()
        with self.assertRaises(PermissionError):
            NoticeReceiver(search, peer, adv_ep, renderer, identity)

    def test_ssdp_malformed_duplicate_and_oversized_input(self):
        for data in (b"a" * 2049, b"HTTP/1.1 200 OK\r\nST: a\r\nST: b\r\n\r\n",
                     b"HTTP/1.1 200 OK\r\nLOCATION: http://evil/\x00\r\n\r\n"):
            with self.assertRaises(ProtocolError):
                _parse(data)


if __name__ == "__main__":
    unittest.main()
