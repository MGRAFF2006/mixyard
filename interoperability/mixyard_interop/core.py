"""Local admission boundary for optional legacy protocol adapters.

Spec mirrors network/src/protocol.rs, not a new network wire protocol. A Session
is explicit local consent for owned loopback work, never paired-device trust.
"""

from dataclasses import dataclass
import json
import math
import threading
import time


SAMPLE_BYTES = {"raw-s16-le": 2, "raw-s24-le": 3, "raw-s32-le": 4,
                "raw-f32-le": 4, "raw-f64-le": 8}
MEDIA_ROLES = frozenset({"live-send", "live-receive", "media-serve", "media-fetch"})
ROLES = MEDIA_ROLES | {"advertise", "control", "service-control"}


def _identifier(value):
    if not isinstance(value, str) or not value or len(value.encode("utf-8")) > 128:
        raise ValueError("invalid identity/name")
    if any(ord(c) < 32 or 127 <= ord(c) <= 159 for c in value):
        raise ValueError("control character in identity/name")
    return value


def _integer(value, low, high, name):
    if type(value) is not int or not low <= value <= high:
        raise ValueError(f"invalid {name}")


def _fields(value, keys):
    if type(value) is not dict or set(value) != set(keys):
        raise ValueError("missing or unknown Spec fields")


def _tuple(value, name):
    if not isinstance(value, (list, tuple)) or len(value) > 4096:
        raise ValueError(f"invalid/bounded {name}")
    return tuple(value)


@dataclass(frozen=True)
class Source:
    peer: str
    source: str
    groups: tuple[str, ...] = ()

    def __post_init__(self):
        _identifier(self.peer)
        _identifier(self.source)
        groups = _tuple(self.groups, "source groups")
        for group in groups:
            _identifier(group)
        if sum(len(g.encode("utf-8")) + 3 for g in groups) > 4096:
            raise ValueError("source metadata too large")
        if len(set(groups)) != len(groups):
            raise ValueError("duplicate source group")
        object.__setattr__(self, "groups", groups)


@dataclass(frozen=True)
class Provenance:
    sources: tuple[Source, ...]
    visited_peers: tuple[str, ...]

    def __post_init__(self):
        sources = _tuple(self.sources, "sources")
        peers = _tuple(self.visited_peers, "visited peers")
        if not sources or not peers or any(type(s) is not Source for s in sources):
            raise ValueError("missing source provenance")
        for peer in peers:
            _identifier(peer)
        if len(set(peers)) != len(peers):
            raise ValueError("peer loop in provenance")
        if len({(s.peer, s.source) for s in sources}) != len(sources):
            raise ValueError("duplicate source path")
        if any(s.peer not in peers for s in sources):
            raise ValueError("source origin missing from path")
        metadata = sum(len(p.encode("utf-8")) + 3 for p in peers)
        for source in sources:
            metadata += len(source.peer.encode("utf-8")) + len(source.source.encode("utf-8"))
            metadata += sum(len(g.encode("utf-8")) + 3 for g in source.groups) + 40
        if metadata > 4096:
            raise ValueError("provenance metadata too large")
        object.__setattr__(self, "sources", sources)
        object.__setattr__(self, "visited_peers", peers)


@dataclass(frozen=True)
class Spec:
    stream: int
    name: str
    codec: str
    sample_rate: int
    channels: tuple[str, ...]
    frames_per_packet: int
    provenance: Provenance

    def __post_init__(self):
        _integer(self.stream, 1, 2**64 - 1, "stream")
        _identifier(self.name)
        if not isinstance(self.codec, str) or self.codec not in SAMPLE_BYTES:
            raise ValueError("unsupported codec; no implicit conversion")
        _integer(self.sample_rate, 1, 2**32 - 1, "sample rate")
        _integer(self.frames_per_packet, 1, 2**16 - 1, "frames per packet")
        channels = _tuple(self.channels, "channels")
        if not channels:
            raise ValueError("empty channel layout")
        for channel in channels:
            _identifier(channel)
        if len(set(channels)) != len(channels):
            raise ValueError("duplicate channel identifier")
        if type(self.provenance) is not Provenance:
            raise ValueError("invalid provenance")
        object.__setattr__(self, "channels", channels)
        if len(self.to_json()) > 4096:
            raise ValueError("provenance metadata too large")

    @property
    def frame_bytes(self):
        return SAMPLE_BYTES[self.codec] * len(self.channels)

    def to_dict(self):
        return {"stream": self.stream, "name": self.name, "codec": self.codec,
                "sample_rate": self.sample_rate, "channels": list(self.channels),
                "frames_per_packet": self.frames_per_packet,
                "provenance": {"sources": [{"peer": s.peer, "source": s.source,
                                              "groups": list(s.groups)}
                                             for s in self.provenance.sources],
                               "visited_peers": list(self.provenance.visited_peers)}}

    def to_json(self):
        return json.dumps(self.to_dict(), ensure_ascii=False, separators=(",", ":")).encode("utf-8")

    @classmethod
    def from_json(cls, payload):
        if not isinstance(payload, bytes) or len(payload) > 4096:
            raise ValueError("bounded Spec bytes required")
        def unique_pairs(pairs):
            obj = {}
            for key, value in pairs:
                if key in obj:
                    raise ValueError("duplicate JSON key")
                obj[key] = value
            return obj
        try:
            return cls.from_dict(json.loads(payload, object_pairs_hook=unique_pairs))
        except (RecursionError, UnicodeError, json.JSONDecodeError) as exc:
            raise ValueError("invalid bounded Spec JSON") from exc

    @classmethod
    def from_dict(cls, value):
        _fields(value, ("stream", "name", "codec", "sample_rate", "channels",
                        "frames_per_packet", "provenance"))
        p = value["provenance"]
        _fields(p, ("sources", "visited_peers"))
        sources = []
        source_bytes = 0
        for source in _tuple(p["sources"], "sources"):
            _fields(source, ("peer", "source", "groups"))
            parsed = Source(**source)
            source_bytes += len(parsed.peer.encode("utf-8")) + len(parsed.source.encode("utf-8"))
            source_bytes += sum(len(g.encode("utf-8")) + 3 for g in parsed.groups) + 40
            if source_bytes > 4096:
                raise ValueError("source metadata too large")
            sources.append(parsed)
        return cls(value["stream"], value["name"], value["codec"], value["sample_rate"],
                   value["channels"], value["frames_per_packet"],
                   Provenance(tuple(sources), p["visited_peers"]))

    def validate_path(self, sender, receiver, excluded_groups=()):
        _identifier(sender)
        _identifier(receiver)
        if sender == receiver or self.provenance.visited_peers[-1] != sender:
            raise ValueError("invalid sender/provenance")
        if receiver in self.provenance.visited_peers:
            raise ValueError("receiver loop in provenance")
        if any(set(s.groups).intersection(excluded_groups) for s in self.provenance.sources):
            raise PermissionError("source group excluded; reject whole premix")


@dataclass(frozen=True)
class Endpoint:
    host: str
    port: int
    scheme: str = "http"

    def __post_init__(self):
        if self.host != "127.0.0.1" or self.scheme not in {"http", "udp", "tcp", "rtsp"}:
            raise ValueError("only pinned literal owned IPv4 loopback endpoints are admitted")
        _integer(self.port, 1, 65535, "endpoint port")


@dataclass(frozen=True)
class Limits:
    max_grants: int = 64
    max_frame_bytes: int = 65536
    max_bytes_per_second: int = 64 * 1024 * 1024

    def __post_init__(self):
        for name in self.__dataclass_fields__:
            _integer(getattr(self, name), 1, 2**63 - 1, name)


@dataclass(frozen=True)
class Capability:
    """An implemented role and its exact admitted formats, not an ecosystem badge."""
    protocol: str
    role: str
    specs: tuple[Spec, ...] = ()
    evidence: str = "loopback-only"

    def __post_init__(self):
        _identifier(self.protocol)
        if self.role not in ROLES:
            raise ValueError("unknown capability role")
        specs = _tuple(self.specs, "capability specs")
        if any(type(spec) is not Spec for spec in specs):
            raise ValueError("invalid capability spec")
        object.__setattr__(self, "specs", specs)

    def supports(self, spec):
        return spec in self.specs


@dataclass(frozen=True)
class Grant:
    role: str
    endpoint: Endpoint
    peer: str
    spec: Spec | None = None

    def __post_init__(self):
        if self.role not in ROLES or type(self.endpoint) is not Endpoint:
            raise ValueError("invalid grant role/endpoint")
        _identifier(self.peer)
        if self.role in MEDIA_ROLES and type(self.spec) is not Spec:
            raise ValueError("media grant requires an exact Spec")
        if self.spec is not None and type(self.spec) is not Spec:
            raise ValueError("invalid grant Spec")


class Session:
    """Runtime-only explicit local consent; no credentials, discovery or pairing.

    require linearizes admission under a lock. Revoke prevents subsequent admits;
    it cannot recall an OS send/SDK call already admitted. Adapters own bounded
    workers and must check after receive and before releasing media to consumers.
    """
    backendAvailable = False
    audioApplied = False

    def __init__(self, local_peer, grants=(), *, consent=False, lifetime_seconds=60,
                 excluded_groups=(), limits=None):
        _identifier(local_peer)
        if consent is not True:
            raise PermissionError("explicit fresh runtime consent required")
        if (type(lifetime_seconds) not in (int, float) or not math.isfinite(lifetime_seconds)
                or not 0 < lifetime_seconds <= 3600):
            raise ValueError("consent lifetime must be finite and at most one hour")
        self._limits = limits if limits is not None else Limits()
        if type(self._limits) is not Limits:
            raise ValueError("invalid limits")
        grants = _tuple(grants, "grants")
        if len(grants) > self._limits.max_grants or any(type(g) is not Grant for g in grants):
            raise ValueError("grant budget exceeded/invalid grants")
        excluded = _tuple(excluded_groups, "excluded groups")
        for group in excluded:
            _identifier(group)
        self._grants = frozenset(grants)
        self._local_peer = local_peer
        self._excluded = frozenset(excluded)
        for grant in grants:
            if grant.peer == local_peer:
                raise ValueError("peer loop")
            if grant.spec is not None:
                sender, receiver = ((local_peer, grant.peer) if grant.role in {"live-send", "media-serve"}
                                    else (grant.peer, local_peer))
                grant.spec.validate_path(sender, receiver, self._excluded)
                if grant.spec.frame_bytes > self._limits.max_frame_bytes:
                    raise ValueError("frame resource budget exceeded")
                if grant.spec.frame_bytes * grant.spec.sample_rate > self._limits.max_bytes_per_second:
                    raise ValueError("rate resource budget exceeded")
        self._deadline = time.monotonic() + lifetime_seconds
        self._active = True
        self._lock = threading.Lock()

    def _check_locked(self):
        if not self._active or time.monotonic() >= self._deadline:
            raise PermissionError("consent expired or session revoked")

    def check(self):
        with self._lock:
            self._check_locked()

    def require(self, role, endpoint, spec=None):
        if role not in ROLES or type(endpoint) is not Endpoint:
            raise PermissionError("invalid operation role/endpoint")
        with self._lock:
            self._check_locked()
            if not any(g.role == role and g.endpoint == endpoint and g.spec == spec for g in self._grants):
                raise PermissionError("operation not explicitly granted for this endpoint and exact Spec")

    def revoke(self):
        with self._lock:
            self._active = False
