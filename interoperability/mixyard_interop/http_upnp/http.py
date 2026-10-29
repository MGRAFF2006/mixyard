"""Bounded, literal-loopback HTTP media IO. No ambient URL or proxy handling."""
from __future__ import annotations

import base64
import json
import re
import socket
import threading
import time
from dataclasses import dataclass
from typing import Callable, Protocol

from mixyard_interop.core import Endpoint, Spec

MAX_MEDIA = 1024 * 1024
MAX_HEADER = 8192
MAX_SOAP = 16384
TIMEOUT = 2.0
CHUNK = 4096
_PATH = re.compile(r"/[A-Za-z0-9_./-]{0,255}\Z")


class Session(Protocol):
    def require(self, role: str, endpoint: Endpoint, spec: Spec | None = None) -> None: ...


class ProtocolError(ValueError):
    """Unsupported, malformed, oversized or incomplete peer input."""


def endpoint_checked(endpoint: Endpoint, scheme: str = "http") -> None:
    if (endpoint.host != "127.0.0.1" or type(endpoint.port) is not int
            or not 1 <= endpoint.port <= 65535 or endpoint.scheme != scheme):
        raise ProtocolError("literal IPv4 loopback endpoint required")


def path_checked(path: str) -> None:
    if not isinstance(path, str) or not _PATH.fullmatch(path) or ".." in path or path.startswith("//"):
        raise ProtocolError("absolute bounded resource path required; no URL/query/escape")


def url(endpoint: Endpoint, path: str) -> str:
    endpoint_checked(endpoint)
    path_checked(path)
    return f"http://127.0.0.1:{endpoint.port}{path}"


def _remaining(sock: socket.socket, deadline: float) -> None:
    left = deadline - time.monotonic()
    if left <= 0:
        raise TimeoutError("operation deadline")
    sock.settimeout(left)


def _send(sock: socket.socket, data: bytes, deadline: float, check: Callable[[], None]) -> None:
    view = memoryview(data)
    while view:
        check()
        _remaining(sock, deadline)
        sent = sock.send(view[:CHUNK])
        if sent == 0:
            raise ProtocolError("closed during write")
        view = view[sent:]


def _head(sock: socket.socket, deadline: float, check: Callable[[], None], *, request_head=False):
    # One-byte header reads avoid consuming unadmitted body bytes. Work <=8 KiB.
    data = bytearray()
    while not data.endswith(b"\r\n\r\n"):
        check()
        _remaining(sock, deadline)
        part = sock.recv(1)
        if not part:
            raise ProtocolError("incomplete HTTP header")
        data.extend(part)
        if len(data) > MAX_HEADER:
            raise ProtocolError("HTTP header budget")
    try:
        lines = data[:-4].decode("ascii").split("\r\n")
    except UnicodeError as exc:
        raise ProtocolError("non-ASCII HTTP header") from exc
    headers = {}
    for line in lines[1:]:
        key, sep, value = line.partition(":")
        key = key.lower()
        if not sep or not re.fullmatch(r"[a-z0-9-]+", key) or key in headers:
            raise ProtocolError("invalid/duplicate HTTP header")
        value = value.strip(" ")
        if any(ord(c) < 32 or ord(c) == 127 for c in value):
            raise ProtocolError("HTTP control character")
        headers[key] = value
    if "transfer-encoding" in headers or "content-encoding" in headers:
        raise ProtocolError("encoded/chunked HTTP unsupported")
    length = headers.get("content-length", "0" if request_head else "")
    if not re.fullmatch(r"[0-9]{1,7}", length):
        raise ProtocolError("one explicit Content-Length required")
    return lines[0], headers, int(length)


def _body(sock: socket.socket, length: int, deadline: float, check: Callable[[], None]) -> bytes:
    parts = bytearray()
    while len(parts) < length:
        check()
        _remaining(sock, deadline)
        part = sock.recv(min(CHUNK, length - len(parts)))
        if not part:
            raise ProtocolError("truncated body")
        parts.extend(part)
    check()
    return bytes(parts)


def _message(first: str, headers: dict[str, str], body: bytes) -> bytes:
    headers = {**headers, "Content-Length": str(len(body)), "Connection": "close"}
    head = (first + "\r\n" + "".join(f"{k}: {v}\r\n" for k, v in headers.items()) + "\r\n").encode("ascii")
    if len(head) > MAX_HEADER:
        raise ProtocolError("outbound header budget")
    return head + body


def request(session: Session, role: str, endpoint: Endpoint, path: str, *,
            spec: Spec | None = None, method: str = "GET", body: bytes = b"",
            headers: dict[str, str] | None = None, limit: int = MAX_MEDIA):
    endpoint_checked(endpoint)
    path_checked(path)
    if method not in ("GET", "POST") or len(body) > MAX_SOAP or not 0 <= limit <= MAX_MEDIA:
        raise ProtocolError("request budget/method")
    headers = headers or {}
    if any(not re.fullmatch(r"[A-Za-z0-9-]+", k) or not isinstance(v, str)
           or any(ord(c) < 32 or ord(c) > 126 for c in v) for k, v in headers.items()):
        raise ProtocolError("invalid outbound header")
    if any(k.lower() in ("host", "content-length", "connection", "transfer-encoding") for k in headers):
        raise ProtocolError("reserved outbound header")
    check = lambda: session.require(role, endpoint, spec)
    check()
    deadline = time.monotonic() + TIMEOUT
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        _remaining(sock, deadline)
        sock.connect((endpoint.host, endpoint.port))
        _send(sock, _message(f"{method} {path} HTTP/1.1",
              {"Host": f"127.0.0.1:{endpoint.port}", **headers}, body), deadline, check)
        first, response_headers, length = _head(sock, deadline, check)
        if first != "HTTP/1.1 200 OK":
            raise ProtocolError("HTTP failure; redirects are never followed")
        if length > limit:
            raise ProtocolError("response body budget")
        result = _body(sock, length, deadline, check)
    return response_headers, result


class Server:
    """One active request, finite backlog, bounded lifetime per request; owned thread."""
    def __init__(self, session: Session, role: str, dispatch, spec: Spec | None = None, *, endpoint=None):
        self.session, self.role, self.spec, self.dispatch = session, role, spec, dispatch
        self._stop = threading.Event()
        self.errors = 0
        self._sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:
            if endpoint is not None:
                endpoint_checked(endpoint)
            self._sock.bind(("127.0.0.1", endpoint.port if endpoint else 0))
            self.endpoint = Endpoint("127.0.0.1", self._sock.getsockname()[1])
            session.require(role, self.endpoint, spec)
            self._sock.listen(2)
            self._sock.settimeout(0.1)
        except BaseException:
            self._sock.close()
            raise
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def _check(self):
        if self._stop.is_set():
            raise PermissionError("listener closed")
        self.session.require(self.role, self.endpoint, self.spec)

    def _run(self):
        while not self._stop.is_set():
            try:
                sock, _ = self._sock.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            with sock:
                deadline = time.monotonic() + TIMEOUT
                try:
                    self._check()
                    first, headers, length = _head(sock, deadline, self._check, request_head=True)
                    method, path, version = first.split(" ")
                    path_checked(path)
                    if (version != "HTTP/1.1" or method not in ("GET", "POST")
                            or headers.get("host") != f"127.0.0.1:{self.endpoint.port}"
                            or length > (MAX_SOAP if method == "POST" else 0)):
                        raise ProtocolError("invalid request")
                    body = _body(sock, length, deadline, self._check)
                    response_headers, response_body = self.dispatch(method, path, headers, body)
                    if len(response_body) > MAX_MEDIA:
                        raise ProtocolError("response body budget")
                    _send(sock, _message("HTTP/1.1 200 OK", response_headers, response_body), deadline, self._check)
                except (ValueError, PermissionError, OSError, TimeoutError):
                    # No peer-supplied text is logged or reflected; failures close the connection.
                    self.errors += 1

    def close(self):
        self._stop.set()
        self._thread.join(TIMEOUT + 0.5)
        self._sock.close()
        if self._thread.is_alive():
            raise RuntimeError("listener did not stop within deadline")

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()


def content_type(spec: Spec, wire: str) -> str:
    if wire == "raw":
        return "application/vnd.mixyard.pcm"
    if (wire == "l16" and spec.codec == "raw-s16-le" and spec.sample_rate in (44100, 48000)
            and spec.channels in (("FC",), ("FL", "FR"))):
        return f"audio/L16;rate={spec.sample_rate};channels={len(spec.channels)}"
    raise ProtocolError("unsupported wire profile; conversion must be explicit and destination-local")


def _content_type_matches(value: str, spec: Spec, wire: str) -> bool:
    """Compare the bounded selected MIME profile, never relax the admitted PCM."""
    expected = content_type(spec, wire).split(";", 1)[0].lower()
    if not isinstance(value, str) or len(value) > 256:
        return False
    parts = value.split(";")
    if parts[0].strip(" ").lower() != expected or len(parts) > 3:
        return False
    if wire == "raw":
        return len(parts) == 1
    parameters = {}
    for part in parts[1:]:
        match = re.fullmatch(r'([A-Za-z]+)=(?:([0-9]{1,10})|"((?:[0-9]|\\[0-9]){1,10})")', part.strip(" "))
        if match is None:
            return False
        name = match[1].lower()
        if name not in ("rate", "channels") or name in parameters:
            return False
        parameters[name] = int((match[2] or match[3]).replace("\\", ""))
    # RFC 2586 requires rate and defaults absent channels to mono.
    return (parameters.get("rate") == spec.sample_rate
            and parameters.get("channels", 1) == len(spec.channels))


def _swap16(data: bytes) -> bytes:
    result = bytearray(data)
    result[0::2], result[1::2] = data[1::2], data[0::2]
    return bytes(result)


def _spec_header(spec: Spec) -> str:
    return base64.b64encode(json.dumps(spec.to_dict(), separators=(",", ":"), ensure_ascii=True).encode()).decode()


@dataclass(frozen=True)
class Resource:
    endpoint: Endpoint
    path: str
    spec: Spec
    wire: str = "raw"

    def __post_init__(self):
        endpoint_checked(self.endpoint)
        path_checked(self.path)
        content_type(self.spec, self.wire)

    @property
    def uri(self):
        return url(self.endpoint, self.path)


class MediaSource(Server):
    """Publish one exact bounded PCM block/resource, with independent live/media role."""
    def __init__(self, session: Session, spec: Spec, pcm: bytes, *, wire="raw", mode="media", endpoint=None):
        if mode not in ("media", "live") or not isinstance(pcm, bytes) or not pcm or len(pcm) > MAX_MEDIA:
            raise ProtocolError("media block budget/mode")
        if len(pcm) % spec.frame_bytes:
            raise ProtocolError("incomplete PCM frame")
        mime = content_type(spec, wire)
        metadata = _spec_header(spec)
        if len(metadata) > 6000:
            raise ProtocolError("metadata budget")
        payload = _swap16(pcm) if wire == "l16" else pcm

        def dispatch(method, path, headers, body):
            if method != "GET" or path != "/pcm":
                raise ProtocolError("unknown media resource")
            return {"Content-Type": mime, "X-Mixyard-Spec": metadata,
                    "X-Mixyard-Mode": mode}, payload

        super().__init__(session, "media-serve" if mode == "media" else "live-send", dispatch, spec, endpoint=endpoint)
        self.resource = Resource(self.endpoint, "/pcm", spec, wire)


class HttpReceiver:
    def __init__(self, session: Session):
        self.session = session

    def fetch(self, resource: Resource, *, mode="media") -> bytes:
        if mode not in ("media", "live"):
            raise ProtocolError("unknown receive mode")
        role = "media-fetch" if mode == "media" else "live-receive"
        headers, pcm = request(self.session, role, resource.endpoint, resource.path, spec=resource.spec)
        if (not _content_type_matches(headers.get("content-type"), resource.spec, resource.wire)
                or headers.get("x-mixyard-mode", mode) != mode
                or headers.get("x-mixyard-spec", _spec_header(resource.spec)) != _spec_header(resource.spec)):
            raise ProtocolError("media identity/format/mode differs from explicit preadmission")
        if not pcm or len(pcm) % resource.spec.frame_bytes:
            raise ProtocolError("incomplete PCM frame")
        self.session.require(role, resource.endpoint, resource.spec)
        return _swap16(pcm) if resource.wire == "l16" else pcm
