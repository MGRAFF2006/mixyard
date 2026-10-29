"""Private bounded literal-loopback TCP control transport, without DNS/retries."""

import socket
import time

from ..core import Endpoint, Session
from .rtp import _endpoint, _timeout

MAX_CONTROL_BYTES = 32768


class ControlError(ValueError):
    """Control peer violated the selected wire profile."""


class TcpControl:
    def __init__(self, session: Session, endpoint: Endpoint, role: str,
                 scheme: str, timeout: float):
        _endpoint(endpoint, scheme)
        _timeout(timeout)
        self.session, self.endpoint, self.role = session, endpoint, role
        self.timeout = timeout
        self._socket: socket.socket | None = None
        self._requests = 0
        self._buffer = bytearray()

    def begin(self) -> None:
        self.session.require(self.role, self.endpoint)
        if self._requests >= 64:
            raise RuntimeError("control lifetime request budget exhausted")
        self._requests += 1
        self._deadline = time.monotonic() + self.timeout
        self._read_bytes = self._io_count = 0
        if self._socket is None:
            self._socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            self._guard()
            self._socket.connect((self.endpoint.host, self.endpoint.port))

    def _guard(self) -> None:
        self.session.require(self.role, self.endpoint)
        remaining = self._deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("control operation deadline exceeded")
        if self._io_count >= 128:
            raise ControlError("control IO work budget exceeded")
        self._io_count += 1
        self._socket.settimeout(remaining)

    def send(self, data: bytes) -> None:
        if len(data) > MAX_CONTROL_BYTES:
            raise ControlError("request byte budget exceeded")
        view = memoryview(data)
        while view:
            self._guard()
            sent = self._socket.send(view)
            if sent <= 0:
                raise ConnectionError("control connection closed during send")
            view = view[sent:]

    def _read(self) -> None:
        self._guard()
        remaining = MAX_CONTROL_BYTES - self._read_bytes
        if remaining <= 0:
            raise ControlError("response byte budget exceeded")
        chunk = self._socket.recv(min(4096, remaining))
        self.session.require(self.role, self.endpoint)
        if not chunk:
            raise ConnectionError("control connection closed before complete response")
        self._read_bytes += len(chunk)
        self._buffer.extend(chunk)

    def until(self, delimiter: bytes, limit: int) -> bytes:
        while True:
            pos = self._buffer.find(delimiter)
            if pos >= 0:
                size = pos + len(delimiter)
                if size > limit:
                    raise ControlError("response framing limit exceeded")
                result = bytes(self._buffer[:size])
                del self._buffer[:size]
                return result
            if len(self._buffer) >= limit:
                raise ControlError("response framing limit exceeded")
            self._read()

    def exact(self, size: int) -> bytes:
        while len(self._buffer) < size:
            self._read()
        result = bytes(self._buffer[:size])
        del self._buffer[:size]
        return result

    def finish(self) -> None:
        self.session.require(self.role, self.endpoint)
        if time.monotonic() >= self._deadline:
            raise TimeoutError("control operation deadline exceeded before release")

    def close(self) -> None:
        if self._socket is not None:
            self._socket.close()
            self._socket = None
        self._buffer.clear()

    def __enter__(self):
        return self

    def __exit__(self, *_exc):
        self.close()
