"""An intentionally partial AVTransport:1 service; never a DLNA certification claim."""
from __future__ import annotations

import re
import threading
import time
import uuid
import xml.etree.ElementTree as ET
from types import MappingProxyType

from .http import (HttpReceiver, MAX_SOAP, ProtocolError, Resource, Server,
                   Session, content_type, request)
from mixyard_interop.core import Endpoint

SOAP = "http://schemas.xmlsoap.org/soap/envelope/"
AVT = "urn:schemas-upnp-org:service:AVTransport:1"
DEVICE = "urn:schemas-upnp-org:device:MediaRenderer:1"
DIDL = "urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/"
DC = "http://purl.org/dc/elements/1.1/"
UPNP = "urn:schemas-upnp-org:device-1-0"
_ACTIONS = {"SetAVTransportURI": ("InstanceID", "CurrentURI", "CurrentURIMetaData"),
            "Play": ("InstanceID", "Speed"), "Stop": ("InstanceID",),
            "GetTransportInfo": ("InstanceID",)}


def validate_identity(identity):
    try:
        if not isinstance(identity, str) or str(uuid.UUID(identity)) != identity:
            raise ValueError("noncanonical UUID")
    except (ValueError, AttributeError) as exc:
        raise ProtocolError("canonical UUID required for UPnP identity") from exc


def xml(data: bytes):
    if not isinstance(data, bytes) or len(data) > MAX_SOAP or b"\x00" in data or b"<!" in data:
        raise ProtocolError("XML budget/declaration")
    try:
        text = data.decode("utf-8", errors="strict")
        root = ET.fromstring(text)
    except (UnicodeError, ET.ParseError) as exc:
        raise ProtocolError("invalid UTF-8 XML") from exc
    pending = [(root, 1)]
    count = 0
    while pending:
        element, depth = pending.pop()
        count += 1
        if count > 128 or depth > 8 or len(element.attrib) > 8:
            raise ProtocolError("XML structure budget")
        pending.extend((child, depth + 1) for child in element)
    return root


def envelope(action: str, args: dict[str, str], *, response=False) -> bytes:
    root = ET.Element(f"{{{SOAP}}}Envelope")
    body = ET.SubElement(root, f"{{{SOAP}}}Body")
    call = ET.SubElement(body, f"{{{AVT}}}{action}{'Response' if response else ''}")
    for name, value in args.items():
        ET.SubElement(call, name).text = value
    data = ET.tostring(root, encoding="utf-8")
    if len(data) > MAX_SOAP:
        raise ProtocolError("SOAP body budget")
    return data


def parse_envelope(data: bytes, action: str, *, response=False):
    root = xml(data)
    if (root.tag != f"{{{SOAP}}}Envelope" or len(root) != 1
            or root[0].tag != f"{{{SOAP}}}Body" or len(root[0]) != 1):
        raise ProtocolError("SOAP envelope/body")
    call = root[0][0]
    if call.tag != f"{{{AVT}}}{action}{'Response' if response else ''}" or call.attrib:
        raise ProtocolError("SOAP action mismatch")
    args = {}
    for child in call:
        if child.tag in args or child.attrib or len(child):
            raise ProtocolError("SOAP argument structure")
        args[child.tag] = child.text or ""
    return args


def metadata(identity: str, resource: Resource) -> str:
    root = ET.Element(f"{{{DIDL}}}DIDL-Lite")
    item = ET.SubElement(root, f"{{{DIDL}}}item", id=identity, parentID="0", restricted="1")
    ET.SubElement(item, f"{{{DC}}}title").text = resource.spec.name
    ET.SubElement(item, f"{{{DIDL}}}res", protocolInfo=f"http-get:*:{content_type(resource.spec, resource.wire)}:*").text = resource.uri
    return ET.tostring(root, encoding="unicode")


def source_identity(text: str, resource: Resource) -> str:
    root = xml(text.encode("utf-8"))
    if root.tag != f"{{{DIDL}}}DIDL-Lite" or len(root) != 1:
        raise ProtocolError("one DIDL source required")
    item = root[0]
    if item.tag != f"{{{DIDL}}}item" or len(item) != 2:
        raise ProtocolError("DIDL item")
    title, res = item
    if (title.tag != f"{{{DC}}}title" or len(title) or title.attrib
            or title.text != resource.spec.name or res.tag != f"{{{DIDL}}}res" or len(res)
            or res.text != resource.uri or res.attrib != {
                "protocolInfo": f"http-get:*:{content_type(resource.spec, resource.wire)}:*"}
            or item.attrib != {"id": item.get("id"), "parentID": "0", "restricted": "1"}):
        raise ProtocolError("DIDL identity/format")
    return item.get("id")


class Renderer(Server):
    """Receive approved media into a bounded PCM buffer; no physical audio output.

    SetAVTransportURI selects, Play fetches one finite resource, Stop clears it.
    Finite-resource completion is STOPPED, never a fabricated PLAYING state.
    """
    def __init__(self, session: Session, sources: dict[str, Resource], *, identity=None, endpoint=None):
        identity = str(uuid.uuid4()) if identity is None else identity
        validate_identity(identity)
        if not 1 <= len(sources) <= 32:
            raise ProtocolError("renderer identity/source budget")
        if any(not re.fullmatch(r"[A-Za-z0-9_-]{1,64}", k) for k in sources):
            raise ProtocolError("source identity")
        self.sources = MappingProxyType(dict(sources))
        self.identity = identity
        self._selected = None
        self._state_lock = threading.RLock()
        self._pcm = b""
        self.state = "NO_MEDIA_PRESENT"
        super().__init__(session, "control", self._dispatch, endpoint=endpoint)

    def take_pcm(self) -> tuple[Resource, bytes]:
        """Revalidate independent media permission at consumer handoff, then drain."""
        with self._state_lock:
            return self._take_pcm()

    def _take_pcm(self):
        self._check()
        if self._selected is None:
            raise ProtocolError("no source selected")
        resource = self.sources[self._selected]
        try:
            self.session.require("media-fetch", resource.endpoint, resource.spec)
        except PermissionError:
            self._pcm = b""
            raise
        pcm, self._pcm = self._pcm, b""
        return resource, pcm

    def _description(self):
        root = ET.Element("root", xmlns=UPNP)
        version = ET.SubElement(root, "specVersion")
        ET.SubElement(version, "major").text = "1"
        ET.SubElement(version, "minor").text = "0"
        device = ET.SubElement(root, "device")
        for name, value in (("deviceType", DEVICE), ("friendlyName", self.identity),
                            ("manufacturer", "Mixyard"), ("modelName", "Bounded loopback subset"),
                            ("UDN", f"uuid:{self.identity}")):
            ET.SubElement(device, name).text = value
        service = ET.SubElement(ET.SubElement(device, "serviceList"), "service")
        for name, value in (("serviceType", AVT), ("serviceId", "urn:upnp-org:serviceId:AVTransport"),
                            ("SCPDURL", "/avtransport.xml"), ("controlURL", "/control"), ("eventSubURL", "")):
            ET.SubElement(service, name).text = value
        return ET.tostring(root, encoding="utf-8")

    def _scpd(self):
        root = ET.Element("scpd", xmlns="urn:schemas-upnp-org:service-1-0")
        version = ET.SubElement(root, "specVersion")
        ET.SubElement(version, "major").text = "1"
        ET.SubElement(version, "minor").text = "0"
        actions = ET.SubElement(root, "actionList")
        variables = {}
        for name, arguments in _ACTIONS.items():
            action = ET.SubElement(actions, "action")
            ET.SubElement(action, "name").text = name
            entries = [(arg, "in") for arg in arguments]
            if name == "GetTransportInfo":
                entries += [(arg, "out") for arg in ("CurrentTransportState", "CurrentTransportStatus", "CurrentSpeed")]
            args = ET.SubElement(action, "argumentList")
            for arg, direction in entries:
                variable = {"InstanceID": "A_ARG_TYPE_InstanceID", "CurrentURI": "AVTransportURI",
                            "CurrentURIMetaData": "AVTransportURIMetaData", "Speed": "TransportPlaySpeed",
                            "CurrentSpeed": "TransportPlaySpeed", "CurrentTransportState": "TransportState",
                            "CurrentTransportStatus": "TransportStatus"}[arg]
                variables[variable] = "ui4" if arg == "InstanceID" else "string"
                argument = ET.SubElement(args, "argument")
                for tag, value in (("name", arg), ("direction", direction), ("relatedStateVariable", variable)):
                    ET.SubElement(argument, tag).text = value
        table = ET.SubElement(root, "serviceStateTable")
        for name, datatype in variables.items():
            variable = ET.SubElement(table, "stateVariable", sendEvents="no")
            ET.SubElement(variable, "name").text = name
            ET.SubElement(variable, "dataType").text = datatype
        return ET.tostring(root, encoding="utf-8")

    def _dispatch(self, method, path, headers, body, *, deadline):
        with self._state_lock:
            return self._handle(method, path, headers, body, deadline=deadline)

    def _handle(self, method, path, headers, body, *, deadline):
        if method == "GET" and path in ("/device.xml", "/avtransport.xml"):
            return {"Content-Type": "text/xml; charset=utf-8"}, self._description() if path == "/device.xml" else self._scpd()
        if method != "POST" or path != "/control" or headers.get("content-type") != "text/xml; charset=utf-8":
            raise ProtocolError("unsupported control resource")
        soap_action = headers.get("soapaction", "")
        action = next((name for name in _ACTIONS if soap_action == f'"{AVT}#{name}"'), None)
        if action is None:
            raise ProtocolError("unsupported SOAP action")
        args = parse_envelope(body, action)
        if set(args) != set(_ACTIONS[action]) or args["InstanceID"] != "0":
            raise ProtocolError("unsupported arguments/instance")
        result = {}
        if action == "SetAVTransportURI":
            didl = xml(args["CurrentURIMetaData"].encode("utf-8"))
            if didl.tag != f"{{{DIDL}}}DIDL-Lite" or len(didl) != 1:
                raise ProtocolError("one explicit DIDL source required")
            selected = didl[0].get("id")
            resource = self.sources.get(selected)
            if resource is None or resource.uri != args["CurrentURI"]:
                raise ProtocolError("URL and source identity require independent explicit approval")
            source_identity(args["CurrentURIMetaData"], resource)
            self.session.require("media-fetch", resource.endpoint, resource.spec)
            self._selected, self._pcm, self.state = selected, b"", "STOPPED"
        elif action == "Play":
            if args["Speed"] != "1" or self._selected is None:
                raise ProtocolError("unsupported speed/no selected source")
            self._pcm = b""
            resource = self.sources[self._selected]
            try:
                pcm = HttpReceiver(self.session).fetch(resource, deadline=deadline)
                self._check()
                self.session.require("media-fetch", resource.endpoint, resource.spec)
                if time.monotonic() >= deadline:
                    raise TimeoutError("renderer request deadline")
                self._pcm = pcm
            finally:
                self.state = "STOPPED"
        elif action == "Stop":
            self._pcm = b""
            self.state = "STOPPED" if self._selected is not None else "NO_MEDIA_PRESENT"
        else:
            result = {"CurrentTransportState": self.state, "CurrentTransportStatus": "OK", "CurrentSpeed": "1"}
        return {"Content-Type": "text/xml; charset=utf-8"}, envelope(action, result, response=True)

    def close(self):
        super().close()
        self._pcm = b""
        self._selected = None
        self.state = "NO_MEDIA_PRESENT"


class Controller:
    def __init__(self, session: Session, endpoint: Endpoint):
        self.session, self.endpoint = session, endpoint

    def action(self, action: str, **arguments):
        if action not in _ACTIONS or set(arguments) != set(_ACTIONS[action]) or any(not isinstance(v, str) for v in arguments.values()):
            raise ProtocolError("unsupported controller action/arguments")
        headers, result = request(self.session, "control", self.endpoint, "/control", method="POST",
                                  headers={"Content-Type": "text/xml; charset=utf-8", "SOAPAction": f'"{AVT}#{action}"'},
                                  body=envelope(action, arguments), limit=MAX_SOAP)
        if headers.get("content-type") != "text/xml; charset=utf-8":
            raise ProtocolError("control response type")
        args = parse_envelope(result, action, response=True)
        expected = {"CurrentTransportState", "CurrentTransportStatus", "CurrentSpeed"} if action == "GetTransportInfo" else set()
        if set(args) != expected:
            raise ProtocolError("control response arguments")
        return args

    def set_uri(self, identity: str, resource: Resource):
        return self.action("SetAVTransportURI", InstanceID="0", CurrentURI=resource.uri,
                           CurrentURIMetaData=metadata(identity, resource))

    def play(self):
        return self.action("Play", InstanceID="0", Speed="1")

    def stop(self):
        return self.action("Stop", InstanceID="0")

    def info(self):
        return self.action("GetTransportInfo", InstanceID="0")
