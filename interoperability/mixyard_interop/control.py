"""Revision-scoped interop inventory/configuration, without ambient media rights.

ManagementScope is the existing management service's already verified view /
control / settings result. It is not a login or an audio grant. This module is a
local integration API; it does not expose an unauthenticated network service.
"""

from dataclasses import dataclass
import threading

from .core import Endpoint, MEDIA_ROLES, ROLES, Session, Spec, _identifier, _integer
from .runtime import RuntimeLease


@dataclass(frozen=True)
class ManagementScope:
    device_id: str
    authority_id: str
    revision: int
    permissions: frozenset[str]

    def __post_init__(self):
        _identifier(self.device_id)
        _identifier(self.authority_id)
        _integer(self.revision, 1, 2**64 - 1, "management revision")
        rights = frozenset(self.permissions)
        if not rights <= {"view", "control", "settings"}:
            raise ValueError("management DTO permits only view/control/settings")
        object.__setattr__(self, "permissions", rights)


@dataclass(frozen=True)
class Suggestion:
    suggestion_id: str
    protocol: str
    label: str
    endpoint: Endpoint

    def __post_init__(self):
        for value in (self.suggestion_id, self.protocol, self.label):
            _identifier(value)
        if type(self.endpoint) is not Endpoint:
            raise ValueError("bounded loopback inventory suggestion required")


@dataclass(frozen=True)
class Selection:
    device_id: str
    authority_id: str
    revision: int
    suggestion: Suggestion
    role: str
    spec: Spec | None


class MediaActivation:
    """A checked directional handoff; invoke is the explicit worker start.

    Neither constructing a Selection nor inspecting this object opens IO.
    Session remains the independent local endpoint/role grant. Callers retain
    worker cleanup ownership; revoking this handle blocks later starts/checks.
    """

    def __init__(self, registry, selection, lease, session):
        self._registry, self.selection = registry, selection
        self.lease, self.session = lease, session
        self._active = True
        self.check()

    def check(self):
        if not self._active:
            raise PermissionError("interop activation revoked")
        self._registry._current_selection(self.selection)
        self.lease.check()
        # The owner callback can revoke this activation or replace settings.
        # Recheck both after it returns, before the last directional grant check.
        if not self._active:
            raise PermissionError("interop activation revoked during owner observation")
        self._registry._current_selection(self.selection)
        self.lease.check_liveness()
        self.session.require(self.selection.role, self.selection.suggestion.endpoint, self.selection.spec)

    def invoke(self, worker):
        self.check()
        return worker(self)

    def revoke(self):
        self._active = False


class RuntimeRegistry:
    """Bounded inventory and one selected configuration per protocol role.

    Untrusted discovery/vendor values can only be suggestions. Their insertion,
    display and explicit selection never fetch a URL, advertise, listen, or
    obtain a grant. Available capabilities require current runtime attachment.
    """

    backendAvailable = False
    audioApplied = False

    def __init__(self, device_id, authority_id, *, revision=1, max_suggestions=64):
        _identifier(device_id)
        _identifier(authority_id)
        _integer(revision, 1, 2**64 - 1, "management revision")
        _integer(max_suggestions, 1, 256, "inventory budget")
        self._device_id, self._authority_id, self._revision = device_id, authority_id, revision
        self._max_suggestions = max_suggestions
        self._suggestions, self._selections, self._activations = {}, {}, {}
        self._lock = threading.RLock()
        # These are the runtime-connected profiles implemented in this package.
        # Vendor/browser contracts remain inventory-only, never available here.
        self._roles = {"http-pcm": frozenset({"live-send", "live-receive"}),
                       "rtp": frozenset({"live-send", "live-receive"})}

    def _authorize(self, scope, right):
        if (type(scope) is not ManagementScope
                or (scope.device_id, scope.authority_id, scope.revision)
                != (self._device_id, self._authority_id, self._revision)):
            raise PermissionError("stale or different device/authority/revision")
        if right not in scope.permissions:
            raise PermissionError(f"management {right} permission required")

    def add_suggestion(self, suggestion):
        """Inventory adapter ingress only; no peer trust or service activation."""
        if type(suggestion) is not Suggestion:
            raise ValueError("validated inventory suggestion required")
        with self._lock:
            previous = self._suggestions.get(suggestion.suggestion_id)
            if previous is not None and previous != suggestion:
                raise ValueError("suggestion identity changed; add a new identity")
            if previous is None and len(self._suggestions) >= self._max_suggestions:
                raise ValueError("inventory suggestion budget exhausted")
            self._suggestions[suggestion.suggestion_id] = suggestion

    def snapshot(self, scope):
        with self._lock:
            self._authorize(scope, "view")
            capabilities = []
            for protocol, roles in self._roles.items():
                states = []
                for role in sorted(roles):
                    activation = self._activations.get((protocol, role))
                    try:
                        if activation is None:
                            raise PermissionError("no current runtime attachment")
                        activation.check()
                        available, reason = True, "explicit owned loopback runtime lease"
                    except PermissionError as exc:
                        available, reason = False, str(exc)
                    states.append({"role": role, "available": available, "reason": reason})
                capabilities.append({"protocol": protocol, "roles": states,
                                     "evidence": "owned-loopback", "nativeAvailable": False,
                                     "physicalQualified": False})
            self._authorize(scope, "view")
            return {"deviceId": self._device_id, "authorityId": self._authority_id,
                    "revision": self._revision, "capabilities": capabilities,
                    "suggestions": [{"suggestionId": s.suggestion_id, "protocol": s.protocol,
                                     "label": s.label, "endpoint": {"host": s.endpoint.host,
                                     "port": s.endpoint.port, "scheme": s.endpoint.scheme},
                                     "untrusted": True} for s in self._suggestions.values()]}

    def select(self, scope, suggestion_id, role, spec=None):
        return self.configure(scope, ((suggestion_id, role, spec),))[0]

    def configure(self, scope, entries):
        """Replace the complete selection atomically; permits both directions.

        A new configuration invalidates old pending/active handoffs. Reattachment
        is explicit with the still-current runtime lease and directional grants.
        """
        entries = tuple(entries)
        if not 1 <= len(entries) <= 32:
            raise ValueError("bounded nonempty configuration required")
        with self._lock:
            self._authorize(scope, "settings")
            accepted, keys = [], set()
            for suggestion_id, role, spec in entries:
                suggestion = self._suggestions.get(suggestion_id)
                if suggestion is None or role not in ROLES:
                    raise ValueError("unknown suggestion/role")
                if role in MEDIA_ROLES and type(spec) is not Spec:
                    raise ValueError("exact canonical media Spec required")
                if spec is not None and type(spec) is not Spec:
                    raise ValueError("invalid canonical Spec")
                key = (suggestion.protocol, role)
                if key in keys:
                    raise ValueError("duplicate protocol role configuration")
                keys.add(key)
                accepted.append((suggestion, role, spec))
            if self._revision == 2**64 - 1:
                raise ValueError("management revision exhausted")
            self._revision += 1
            selections = tuple(Selection(self._device_id, self._authority_id, self._revision,
                                         suggestion, role, spec)
                               for suggestion, role, spec in accepted)
            self._selections = {(s.suggestion.protocol, s.role): s for s in selections}
            self._activations.clear()
            return selections

    def _current_selection(self, selection):
        with self._lock:
            if (type(selection) is not Selection
                    or (selection.device_id, selection.authority_id, selection.revision)
                    != (self._device_id, self._authority_id, self._revision)
                    or self._selections.get((selection.suggestion.protocol, selection.role)) != selection):
                raise PermissionError("interop selection stale or belongs to another authority")

    def attach(self, selection, lease, session):
        """Explicit runtime-owner action, never called by select/settings."""
        with self._lock:
            self._current_selection(selection)
            protocol, role = selection.suggestion.protocol, selection.role
            if role not in self._roles.get(protocol, ()):
                raise PermissionError("adapter role has no runtime integration")
            scheme = "http" if protocol == "http-pcm" else "udp"
            if selection.suggestion.endpoint.scheme != scheme:
                raise ValueError("selected endpoint scheme differs from adapter")
            if type(lease) is not RuntimeLease or type(session) is not Session or lease.spec != selection.spec:
                raise PermissionError("exact runtime lease and separate local directional grant required")
            if protocol == "rtp":
                from .realtime.rtp import RtpProfile
                RtpProfile(selection.spec)  # Exact implemented encoding/packet budget; no IO.
            activation = MediaActivation(self, selection, lease, session)
            self._activations[(protocol, role)] = activation
            return activation
