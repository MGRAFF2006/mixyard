"""Snapcast public TCP JSON-RPC 2.0 control, without PCM or sync claims."""

import json
import math

from ..core import Endpoint, Session
from ._tcp import ControlError, TcpControl
from .rtp import _integer


def _identifier(value: str) -> str:
    if type(value) is not str or not 1 <= len(value) <= 128 or not value.isascii() or any(ord(c) < 32 for c in value):
        raise ValueError("bounded ASCII Snapcast identifier required")
    return value


def _object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ControlError("duplicate JSON member")
        result[key] = value
    return result


def _json(raw: bytes):
    try:
        text = raw.decode("utf-8")
    except UnicodeError as exc:
        raise ControlError("UTF-8 JSON response required") from exc
    depth = 0
    quoted = escaped = False
    for octet in raw:
        if quoted:
            if escaped:
                escaped = False
            elif octet == 92:
                escaped = True
            elif octet == 34:
                quoted = False
        elif octet == 34:
            quoted = True
        elif octet in (91, 123):
            depth += 1
            if depth > 16:
                raise ControlError("JSON depth budget exceeded")
        elif octet in (93, 125):
            depth -= 1
    def number(value):
        if len(value) > 20:
            raise ControlError("JSON integer budget exceeded")
        return int(value)
    def constant(_value):
        raise ControlError("non-finite JSON value")
    def fraction(value):
        if len(value) > 64:
            raise ControlError("JSON fraction budget exceeded")
        result = float(value)
        if not math.isfinite(result):
            raise ControlError("non-finite JSON fraction")
        return result
    try:
        return json.loads(text, object_pairs_hook=_object, parse_int=number,
                          parse_float=fraction, parse_constant=constant)
    except (ValueError, UnicodeError, RecursionError) as exc:
        raise ControlError("invalid bounded JSON response") from exc


class SnapcastControl(TcpControl):
    def __init__(self, session: Session, endpoint: Endpoint, *, timeout: float = 0.5):
        super().__init__(session, endpoint, "service-control", "tcp", timeout)

    def _call(self, method: str, params: dict | None = None):
        try:
            self.begin()
            request_id = self._requests
            request = {"id": request_id, "jsonrpc": "2.0", "method": method}
            if params is not None:
                request["params"] = params
            self.send(json.dumps(request, separators=(",", ":"), allow_nan=False).encode("ascii") + b"\n")
            # At most eight notifications before the correlated response; no queue.
            for _ in range(9):
                response = _json(self.until(b"\n", 16384))
                if not isinstance(response, dict) or response.get("jsonrpc") != "2.0":
                    raise ControlError("JSON-RPC 2.0 object required")
                if "id" not in response and type(response.get("method")) is str:
                    continue
                if type(response.get("id")) is not int or response["id"] != request_id:
                    raise ControlError("mismatched JSON-RPC response ID")
                if ("result" in response) == ("error" in response):
                    raise ControlError("exactly one JSON-RPC result or error required")
                if "error" in response:
                    raise ControlError("Snapcast returned a JSON-RPC error; operation not retried")
                self.finish()
                return response["result"]
            raise ControlError("notification work budget exceeded")
        except BaseException:
            self.close()
            raise

    def get_rpc_version(self):
        return self._call("Server.GetRPCVersion")

    def get_status(self):
        return self._call("Server.GetStatus")

    def get_client_status(self, client_id: str):
        return self._call("Client.GetStatus", {"id": _identifier(client_id)})

    def set_client_volume(self, client_id: str, *, percent: int, muted: bool):
        _integer(percent, 0, 100, "volume percent")
        if type(muted) is not bool:
            raise ValueError("explicit boolean mute required")
        return self._call("Client.SetVolume", {"id": _identifier(client_id),
                          "volume": {"muted": muted, "percent": percent}})

    def set_group_mute(self, group_id: str, *, muted: bool):
        if type(muted) is not bool:
            raise ValueError("explicit boolean mute required")
        return self._call("Group.SetMute", {"id": _identifier(group_id), "mute": muted})
