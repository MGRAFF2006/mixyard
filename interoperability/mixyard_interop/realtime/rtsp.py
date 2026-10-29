"""Explicit RTSP/1.0 UDP-unicast playback control client (RFC 2326).

This control role neither opens RTP sockets nor admits formats or media peers.
"""

from dataclasses import dataclass
import re
from types import MappingProxyType
from typing import Mapping

from ..core import Endpoint, Session
from ._tcp import ControlError, TcpControl
from .rtp import _endpoint


def _path(value: str) -> str:
    if type(value) is not str or len(value) > 512 or not re.fullmatch(r"/[A-Za-z0-9/_.~%+=-]*", value):
        raise ValueError("explicit ASCII RTSP resource path required")
    return value


@dataclass(frozen=True)
class RtspResponse:
    status: int
    headers: Mapping[str, str]
    body: bytes


class RtspClient(TcpControl):
    def __init__(self, session: Session, endpoint: Endpoint, path: str,
                 *, timeout: float = 0.5):
        super().__init__(session, endpoint, "control", "rtsp", timeout)
        self.path = _path(path)
        self._session_id: str | None = None
        self.state = "init"
        self.server_ports: tuple[int, int] | None = None

    def _request(self, method: str, *, path: str | None = None,
                 transport: str | None = None) -> RtspResponse:
        resource = self.path if path is None else _path(path)
        url = f"rtsp://{self.endpoint.host}:{self.endpoint.port}{resource}"
        try:
            self.begin()
            cseq = self._requests
            lines = [f"{method} {url} RTSP/1.0", f"CSeq: {cseq}", "User-Agent: Mixyard-interoperability"]
            if self._session_id is not None:
                lines.append(f"Session: {self._session_id}")
            if method == "DESCRIBE":
                lines.append("Accept: application/sdp")
            if transport is not None:
                lines.append(f"Transport: {transport}")
            self.send(("\r\n".join(lines) + "\r\n\r\n").encode("ascii"))
            head = self.until(b"\r\n\r\n", 8192)
            try:
                rows = head.decode("ascii").split("\r\n")
            except UnicodeDecodeError as exc:
                raise ControlError("non-ASCII RTSP header") from exc
            match = re.fullmatch(r"RTSP/1\.0 ([1-5][0-9]{2}) [\x20-\x7e]*", rows[0])
            if match is None or len(rows) > 67:
                raise ControlError("unsupported RTSP status line or header count")
            headers = {}
            for row in rows[1:-2]:
                key, sep, value = row.partition(":")
                key = key.lower()
                if not sep or not re.fullmatch(r"[a-z0-9-]+", key) or key in headers:
                    raise ControlError("invalid or duplicate RTSP header")
                if any(ord(char) < 32 or ord(char) > 126 for char in value):
                    raise ControlError("invalid RTSP header value")
                headers[key] = value.strip()
            if headers.get("cseq") != str(cseq):
                raise ControlError("mismatched RTSP CSeq")
            if "transfer-encoding" in headers:
                raise ControlError("RTSP transfer encoding unsupported")
            length = headers.get("content-length", "0")
            if not re.fullmatch(r"[0-9]{1,5}", length) or int(length) > 16384:
                raise ControlError("RTSP body byte budget exceeded")
            body = self.exact(int(length))
            if self._buffer:
                raise ControlError("unsolicited/pipelined RTSP bytes unsupported")
            status = int(match[1])
            if not 200 <= status < 300:
                # In particular, never follow Location or authentication redirects.
                raise ControlError(f"RTSP peer returned {status}; no retry or redirect")
            if "session" in headers:
                session_id = headers["session"].split(";", 1)[0]
                if not re.fullmatch(r"[A-Za-z0-9$_.+-]{1,128}", session_id):
                    raise ControlError("invalid RTSP session ID")
                if self._session_id is not None and session_id != self._session_id:
                    raise ControlError("RTSP session changed unexpectedly")
            self.finish()
            return RtspResponse(status, MappingProxyType(headers), body)
        except BaseException:
            self.close()
            raise

    def options(self) -> RtspResponse:
        return self._request("OPTIONS")

    def describe(self) -> RtspResponse:
        response = self._request("DESCRIBE")
        if response.headers.get("content-type", "").split(";", 1)[0].lower() != "application/sdp":
            self.close()
            raise ControlError("DESCRIBE requires application/sdp")
        # SDP remains untrusted metadata. No URL or format is automatically admitted.
        return response

    def setup(self, rtp: Endpoint, rtcp: Endpoint, *, track_path: str | None = None) -> RtspResponse:
        if self.state != "init":
            raise ControlError("SETUP requires initial state")
        _endpoint(rtp, "udp")
        _endpoint(rtcp, "udp")
        if rtcp.port != rtp.port + 1:
            raise ValueError("explicit consecutive RTP/RTCP client ports required")
        response = self._request("SETUP", path=track_path,
                                 transport=f"RTP/AVP;unicast;client_port={rtp.port}-{rtcp.port}")
        try:
            session_id = response.headers.get("session", "").split(";", 1)[0]
            if not session_id:
                raise ControlError("SETUP response missing session")
            fields = response.headers.get("transport", "").split(";")
            if fields[0] not in ("RTP/AVP", "RTP/AVP/UDP") or fields.count("unicast") != 1:
                raise ControlError("only explicit UDP unicast transport is supported")
            values = {}
            for field in fields[1:]:
                if field == "unicast":
                    continue
                key, sep, value = field.partition("=")
                if not sep or key in values or key not in ("client_port", "server_port", "ssrc", "mode", "source"):
                    raise ControlError("unsupported or duplicate transport field")
                values[key] = value
            if values.get("client_port") != f"{rtp.port}-{rtcp.port}":
                raise ControlError("server changed admitted client ports")
            if values.get("source", "127.0.0.1") != "127.0.0.1":
                raise ControlError("non-loopback RTP server source rejected")
            if values.get("mode", '"PLAY"') not in ('"PLAY"', "PLAY"):
                raise ControlError("only PLAY transport mode is supported")
            if "ssrc" in values and not re.fullmatch(r"[0-9A-Fa-f]{8}", values["ssrc"]):
                raise ControlError("invalid transport SSRC")
            ports = values.get("server_port", "")
            if not re.fullmatch(r"[0-9]{1,5}-[0-9]{1,5}", ports):
                raise ControlError("explicit server RTP/RTCP ports required")
            first, second = map(int, ports.split("-"))
            if not 1 <= first < second <= 65535 or second != first + 1:
                raise ControlError("invalid server RTP/RTCP ports")
            self.server_ports = (first, second)
            self._session_id = session_id
            self.state = "ready"
            return response
        except BaseException:
            self.close()
            raise

    def play(self) -> RtspResponse:
        if self.state != "ready":
            raise ControlError("PLAY requires ready state")
        response = self._request("PLAY")
        self.state = "playing"
        return response

    def pause(self) -> RtspResponse:
        if self.state != "playing":
            raise ControlError("PAUSE requires playing state")
        response = self._request("PAUSE")
        self.state = "ready"
        return response

    def teardown(self) -> RtspResponse:
        if self.state not in ("ready", "playing"):
            raise ControlError("TEARDOWN requires an established session")
        response = self._request("TEARDOWN")
        self.close()
        return response

    def close(self) -> None:
        super().close()
        self._session_id = None
        self.server_ports = None
        self.state = "init"
