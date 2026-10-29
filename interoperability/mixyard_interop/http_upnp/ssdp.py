"""SSDP syntax over explicitly owned unicast IPv4 loopback endpoints only."""
from __future__ import annotations

import re
import socket
import threading
from dataclasses import dataclass

from mixyard_interop.core import Endpoint
from .http import ProtocolError, Session, TIMEOUT, endpoint_checked, url
from .upnp import DEVICE, validate_identity

MAX_DATAGRAM = 2048


def _parse(data: bytes):
    if len(data) > MAX_DATAGRAM or not data.endswith(b"\r\n\r\n"):
        raise ProtocolError("SSDP datagram budget/terminator")
    try:
        lines = data[:-4].decode("ascii").split("\r\n")
    except UnicodeError as exc:
        raise ProtocolError("SSDP ASCII required") from exc
    fields = {}
    for line in lines[1:]:
        name, sep, value = line.partition(":")
        name, value = name.upper(), value.strip(" ")
        if (not sep or not re.fullmatch(r"[A-Z-]+", name) or name in fields
                or any(ord(c) < 32 or ord(c) > 126 for c in value)):
            raise ProtocolError("SSDP header")
        fields[name] = value
    return lines[0], fields


def _packet(first, fields):
    data = (first + "\r\n" + "".join(f"{k}: {v}\r\n" for k, v in fields.items()) + "\r\n").encode("ascii")
    if len(data) > MAX_DATAGRAM:
        raise ProtocolError("SSDP packet budget")
    return data


@dataclass(frozen=True)
class Discovery:
    identity: str
    location: str
    # Legacy discovery never confers pairing or authorization.
    paired: bool = False
    alive: bool = True


class Advertiser:
    """One pinned search peer; no multicast joins, broadcasts or LAN sockets."""
    def __init__(self, session: Session, renderer: Endpoint, identity: str,
                 search_peer: Endpoint, *, endpoint: Endpoint):
        endpoint_checked(renderer)
        endpoint_checked(search_peer, "udp")
        endpoint_checked(endpoint, "udp")
        validate_identity(identity)
        self.session, self.endpoint, self.peer = session, endpoint, search_peer
        self.renderer, self.identity = renderer, identity
        self.errors = 0
        self._announced = False
        self._stop = threading.Event()
        self._sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            self._check()
            self._sock.bind((endpoint.host, endpoint.port))
            self._sock.settimeout(0.1)
            self._thread = threading.Thread(target=self._run, daemon=True)
            self._thread.start()
        except BaseException:
            self._stop.set()
            self._sock.close()
            raise

    def _check(self):
        if self._stop.is_set():
            raise PermissionError("advertisement stopped")
        self.session.require("advertise", self.endpoint)
        self.session.require("advertise", self.peer)
        self.session.require("advertise", self.renderer)

    def _run(self):
        while not self._stop.is_set():
            try:
                # Check before every receive and send; revocation withdraws responder.
                self._check()
                data, peer = self._sock.recvfrom(MAX_DATAGRAM + 1)
                if peer != (self.peer.host, self.peer.port):
                    continue
                first, fields = _parse(data)
                if (first != "M-SEARCH * HTTP/1.1" or fields != {
                        "HOST": f"127.0.0.1:{self.endpoint.port}", "MAN": '"ssdp:discover"',
                        "MX": "1", "ST": DEVICE}):
                    raise ProtocolError("unsupported SSDP search")
                response = _packet("HTTP/1.1 200 OK", {
                    "CACHE-CONTROL": "max-age=1", "EXT": "", "LOCATION": url(self.renderer, "/device.xml"),
                    "SERVER": "Mixyard/1 UPnP/1.0 Loopback/1", "ST": DEVICE,
                    "USN": f"uuid:{self.identity}::{DEVICE}"})
                self._check()
                self._sock.sendto(response, peer)
            except socket.timeout:
                continue
            except PermissionError:
                break
            except (OSError, ValueError):
                self.errors += 1

    def announce(self, *, alive=True):
        """Opt-in SSDP NOTIFY to the one pinned local discovery peer."""
        if type(alive) is not bool:
            raise ProtocolError("notification state")
        self._check()
        fields = {"HOST": f"127.0.0.1:{self.peer.port}", "NT": DEVICE,
                  "NTS": "ssdp:alive" if alive else "ssdp:byebye",
                  "USN": f"uuid:{self.identity}::{DEVICE}"}
        if alive:
            fields.update({"CACHE-CONTROL": "max-age=1", "LOCATION": url(self.renderer, "/device.xml"),
                           "SERVER": "Mixyard/1 UPnP/1.0 Loopback/1"})
        self._sock.sendto(_packet("NOTIFY * HTTP/1.1", fields), (self.peer.host, self.peer.port))
        self._announced = alive

    def close(self):
        try:
            if self._announced:
                self.announce(alive=False)
        except PermissionError:
            # Revocation disallows even withdrawal IO; max-age is one second.
            pass
        finally:
            self._stop.set()
            try:
                self._thread.join(TIMEOUT)
            finally:
                self._sock.close()
            if self._thread.is_alive():
                raise RuntimeError("SSDP responder did not stop")

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()


def discover(session: Session, advertiser: Endpoint, *, local: Endpoint,
             expected_renderer: Endpoint, expected_identity: str) -> Discovery:
    endpoint_checked(advertiser, "udp")
    endpoint_checked(local, "udp")
    endpoint_checked(expected_renderer)
    validate_identity(expected_identity)
    def check():
        session.require("advertise", advertiser)
        session.require("advertise", local)
    check()
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.bind((local.host, local.port))
        sock.connect((advertiser.host, advertiser.port))
        sock.settimeout(TIMEOUT)
        check()
        sock.send(_packet("M-SEARCH * HTTP/1.1", {"HOST": f"127.0.0.1:{advertiser.port}",
                  "MAN": '"ssdp:discover"', "MX": "1", "ST": DEVICE}))
        check()
        data = sock.recv(MAX_DATAGRAM + 1)
        check()
    first, fields = _parse(data)
    if (first != "HTTP/1.1 200 OK" or set(fields) != {
            "CACHE-CONTROL", "EXT", "LOCATION", "SERVER", "ST", "USN"}
            or fields["CACHE-CONTROL"] != "max-age=1" or fields["EXT"] != ""
            or fields["ST"] != DEVICE or fields["USN"] != f"uuid:{expected_identity}::{DEVICE}"
            or fields["LOCATION"] != url(expected_renderer, "/device.xml")):
        raise ProtocolError("SSDP response differs from explicit endpoint/identity admission")
    return Discovery(expected_identity, fields["LOCATION"])


class NoticeReceiver:
    """Bounded receive of a NOTIFY from the exact preapproved advertiser."""
    def __init__(self, session: Session, local: Endpoint, advertiser: Endpoint,
                 renderer: Endpoint, identity: str):
        endpoint_checked(local, "udp")
        endpoint_checked(advertiser, "udp")
        endpoint_checked(renderer)
        validate_identity(identity)
        self.session, self.local, self.advertiser = session, local, advertiser
        self.renderer, self.identity = renderer, identity
        self._sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            self._check()
            self._sock.bind((local.host, local.port))
            self._sock.connect((advertiser.host, advertiser.port))
            self._sock.settimeout(TIMEOUT)
        except BaseException:
            self._sock.close()
            raise

    def _check(self):
        self.session.require("advertise", self.local)
        self.session.require("advertise", self.advertiser)

    def receive(self) -> Discovery:
        self._check()
        data = self._sock.recv(MAX_DATAGRAM + 1)
        self._check()
        first, fields = _parse(data)
        alive = fields.get("NTS") == "ssdp:alive"
        expected = {"HOST": f"127.0.0.1:{self.local.port}", "NT": DEVICE,
                    "NTS": "ssdp:alive" if alive else "ssdp:byebye",
                    "USN": f"uuid:{self.identity}::{DEVICE}"}
        if alive:
            expected.update({"CACHE-CONTROL": "max-age=1", "LOCATION": url(self.renderer, "/device.xml"),
                             "SERVER": "Mixyard/1 UPnP/1.0 Loopback/1"})
        if first != "NOTIFY * HTTP/1.1" or fields != expected:
            raise ProtocolError("notification differs from exact endpoint/identity admission")
        return Discovery(self.identity, url(self.renderer, "/device.xml"), alive=alive)

    def close(self):
        self._sock.close()

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()
