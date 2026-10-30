"""Explicit hosted-only SDK comparison. No product SDK requirement or authority."""
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import stat
import shutil
import struct
import subprocess
import sys
import tarfile
import time
import urllib.request

VERSION = "1.4.8"
COMMIT = "8b43ed5c910e892d53de5a96aa01e9280a5b6650"
FIX = "08dcd3a83a9cc64c71e5b6d2ca89af51219f4a47"
ARCHIVE_SIZE = 1928742
ARCHIVE_SHA = "6abd8fe4aaf2f435ee1770fc668acebc2e6b69504600861c794d19b2559ba5db"
ARCHIVE_URL = "https://snapshot.debian.org/file/0de10f0d68a9c8c0cc25a6afd482747fd13bc761"
CRITICAL = {
    "meson.build": "70dfcea88d3d11821d9b5267d5012d00d03b5522",
    "meson_options.txt": "dc1b339f25563952deeaed86235c4deb80705184",
    "spa/include/spa/buffer/alloc.h": "dc8f4cc64e4845af96203382b24faf46fe1d74bf",
    "spa/plugins/audiomixer/audiomixer.c": "8cf7cfdffd9861b9e22ce5068a658e8aefc04781",
    "src/modules/module-client-node/client-node.c": "6c2137b0397dbfa6b481cff08166c4c68baed72f",
    "src/modules/module-client-node/remote-node.c": "52134eaa421491bcc828c97c76e2e537f9aa2011",
    "src/pipewire/mem.c": "642a6b78e6e73fe288b6ecc3d05edd55e7b1b4ab",
    "src/pipewire/mem.h": "52abbd54d3b18140c3eceee7bcb12c031c9cf3f6",
    "test/meson.build": "7c0a4d574ee314d9f36bfea0213b80726e32c5a1",
    "test/test-mempool.c": "36f69c5716f1db8af579ebe74b6c3c60e5f6f718",
}
SOURCE = {"version": VERSION, "commit": COMMIT, "stableFix": FIX,
          "archiveSha256": ARCHIVE_SHA, "archiveBytes": ARCHIVE_SIZE, "archiveUrl": ARCHIVE_URL}
LIMIT = 4 * 1024 * 1024
MODULE_NAMES = {"protocol-native", "client-node", "access", "spa-node-factory", "link-factory", "metadata"}


def require_hosted():
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted":
        raise RuntimeError("SDK preparation/execution requires disposable hosted runner")


def read_bounded(path, limit=LIMIT):
    with open(path, "rb") as stream:
        data = bytearray()
        while True:  # proc files can return a short read before actual EOF.
            chunk = stream.read(min(65536, limit + 1 - len(data)))
            if not chunk:
                return bytes(data)
            data.extend(chunk)
            if len(data) > limit:
                raise RuntimeError("comparison observation byte limit exceeded")


def digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as stream:
        size = 0
        for chunk in iter(lambda: stream.read(65536), b""):
            size += len(chunk)
            if size > 256 * 1024 * 1024:
                raise RuntimeError("comparison file size exceeded")
            h.update(chunk)
    return h.hexdigest()


def build_id(path):
    """Bounded ELF64 little-endian identity from an installed owned file, off RT."""
    with open(path, "rb") as file:
        header = file.read(64)
        if not header.startswith(b"\x7fELF"):
            return None
        if len(header) != 64 or header[4:6] != b"\x02\x01":
            raise RuntimeError("unsupported ELF identity")
        phoff = struct.unpack_from("<Q", header, 32)[0]
        entry, count = struct.unpack_from("<HH", header, 54)
        size = os.fstat(file.fileno()).st_size
        if entry != 56 or count > 128 or phoff + entry * count > size:
            raise RuntimeError("invalid ELF program-header bounds")
        file.seek(phoff)
        table = file.read(entry * count)
        for i in range(count):
            kind, _, offset, _, _, length, _, _ = struct.unpack_from("<IIQQQQQQ", table, i * entry)
            if kind != 4:
                continue
            if length > 65536 or offset + length > size:
                raise RuntimeError("invalid ELF note bounds")
            file.seek(offset)
            notes, at = file.read(length), 0
            while at + 12 <= len(notes):
                n, d, t = struct.unpack_from("<III", notes, at)
                at += 12
                ns, ds = (n + 3) & ~3, (d + 3) & ~3
                if ns + ds > len(notes) - at:
                    raise RuntimeError("invalid ELF note length")
                if t == 3 and n == 4 and notes[at:at + n] == b"GNU\0" and 0 < d <= 64:
                    return notes[at + ns:at + ns + d].hex()
                at += ns + ds
    raise RuntimeError("ELF build ID unavailable")


def manifest(root):
    files, links, total = {}, {}, 0
    for path in sorted(root.rglob("*")):
        name, info = path.relative_to(root).as_posix(), path.lstat()
        if len(files) + len(links) >= 20000:
            raise RuntimeError("SDK manifest cardinality exceeded")
        if stat.S_ISLNK(info.st_mode):
            if not path.resolve(strict=True).is_relative_to(root):
                raise RuntimeError("SDK symlink outside private root")
            links[name] = os.readlink(path)
        elif stat.S_ISREG(info.st_mode):
            total += info.st_size
            if total > 512 * 1024 * 1024:
                raise RuntimeError("SDK manifest aggregate size exceeded")
            files[name] = {"sha256": digest(path), "bytes": info.st_size,
                           "mode": stat.S_IMODE(info.st_mode), "buildId": build_id(path)}
        elif not stat.S_ISDIR(info.st_mode):
            raise RuntimeError("non-file SDK manifest object")
    return {"files": files, "symlinks": links, "totalBytes": total}


def write_json(path, value):
    data = json.dumps(value, separators=(",", ":")).encode()
    if len(data) > LIMIT:
        raise RuntimeError("comparison manifest too large")
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as out:
        out.write(data + b"\n")


def extract_verified(archive, destination):
    if archive.stat().st_size != ARCHIVE_SIZE or digest(archive) != ARCHIVE_SHA:
        raise RuntimeError("pinned archive size/hash mismatch before extraction")
    with tarfile.open(archive, "r:bz2") as source:
        members, total, names = [], 0, set()
        for item in source:
            if len(members) >= 20000:
                raise RuntimeError("archive cardinality exceeded")
            members.append(item)
            name = PurePosixPath(item.name)
            if name.is_absolute() or ".." in name.parts or not name.parts or name.parts[0] != "pipewire-1.4.8" or item.name in names:
                raise RuntimeError("archive path/root ambiguity")
            names.add(item.name)
            if not (item.isfile() or item.isdir() or item.issym()):
                raise RuntimeError("unsupported archive object")
            total += item.size
            if total > 256 * 1024 * 1024:
                raise RuntimeError("archive expanded byte limit exceeded")
        # Python 3.12 hosted data filter rejects escaping links/device metadata.
        source.extractall(destination, members=members, filter="data")
    root = destination / "pipewire-1.4.8"
    for name, expected in CRITICAL.items():
        file = root / name
        if file.is_symlink():
            raise RuntimeError("critical source is not a regular blob")
        data = read_bounded(file)
        actual = hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()
        if actual != expected:
            raise RuntimeError("critical release-tree blob mismatch: " + name)
    return root


def prepare(work):
    require_hosted()
    parent = Path(os.environ["RUNNER_TEMP"]).resolve(strict=True)
    work = work.resolve()
    if not work.is_relative_to(parent) or work == parent:
        raise RuntimeError("SDK work must be private runner temporary child")
    work.mkdir(mode=0o700)
    provenance = parent / "fixed-sdk-source-provenance.json"
    proof = json.loads(read_bounded(provenance))
    if proof.get("valid") is not True or proof.get("executedMerge") != os.environ.get("GITHUB_SHA"):
        raise RuntimeError("comparison source provenance unavailable/mismatched")
    archive = work / "source.tar.bz2"
    with urllib.request.urlopen(ARCHIVE_URL, timeout=30) as incoming, archive.open("xb") as out:
        size = 0
        while chunk := incoming.read(65536):
            size += len(chunk)
            if size > ARCHIVE_SIZE:
                raise RuntimeError("archive transport exceeds pinned bytes")
            out.write(chunk)
    source = extract_verified(archive, work)
    inputs = manifest(source)
    write_json(work / "source-inputs.json", {"source": SOURCE, "criticalGitBlobs": CRITICAL, **inputs})
    prefix, build = work / "prefix", work / "build"
    options = ["--buildtype=release", "--wrap-mode=nofallback", "--libdir=lib", "--sysconfdir=etc",
               "-Dauto_features=disabled", "-Ddocs=disabled", "-Dman=disabled", "-Dexamples=disabled",
               "-Dtests=disabled", "-Dinstalled_tests=disabled", "-Dsession-managers=[]",
               "-Dsystemd=disabled", "-Dsystemd-system-service=disabled", "-Dsystemd-user-service=disabled",
               "-Ddbus=disabled", "-Dflatpak=disabled", "-Dlegacy-rtkit=false", "-Dpam-defaults-install=false",
               "-Drlimits-install=false", "-Dpipewire-alsa=disabled", "-Dpipewire-jack=disabled",
               "-Dpipewire-v4l2=disabled", "-Dalsa=disabled", "-Dudev=disabled", "-Dspa-plugins=enabled", "-Dsupport=enabled",
               "-Daudiomixer=enabled", "-Daudioconvert=disabled", "-Dcontrol=disabled",
               "-Daudiotestsrc=disabled", "-Dvideoconvert=disabled", "-Dvideotestsrc=disabled"]
    subprocess.run(["meson", "setup", str(build), str(source), "--prefix=" + str(prefix), *options], check=True)
    installed = json.loads(subprocess.check_output(["meson", "introspect", "--installed", str(build)]))
    if any(not Path(path).resolve().is_relative_to(prefix) for path in installed.values()):
        raise RuntimeError("Meson planned installation outside private prefix")
    subprocess.run(["meson", "compile", "-C", str(build), "-j2"], check=True)
    subprocess.run(["meson", "install", "-C", str(build), "--no-rebuild"], check=True)
    if manifest(source) != inputs:
        raise RuntimeError("build modified pinned source inputs")
    write_json(work / "prefix-manifest.json", manifest(prefix))
    tools = {}
    for command in (["cc", "--version"], ["meson", "--version"], ["ninja", "--version"], ["pkg-config", "--version"]):
        executable = Path(shutil.which(command[0])).resolve(strict=True)
        tools[command[0]] = {"version": subprocess.check_output(command, text=True)[:2048],
                             "sha256": digest(executable), "buildId": build_id(executable)}
    tools["python"] = {"version": sys.version, "sha256": digest(Path(sys.executable).resolve())}
    tools["packages"] = subprocess.check_output(["dpkg-query", "-W", "-f=${Package}=${Version}\n",
        "meson", "ninja-build", "gettext", "libjson-c-dev", "pkg-config", "gcc", "libc6", "binutils", "python3"], text=True)[:8192]
    write_json(work / "descriptor.json", {"schema": 1, "scope": "hosted-only-fixed-sdk-comparison",
        "source": SOURCE, "prefix": str(prefix), "prefixManifestSha256": digest(work / "prefix-manifest.json"),
        "sourceInputsSha256": digest(work / "source-inputs.json"), "projectProvenanceSha256": digest(provenance),
        "buildOptions": options, "tools": tools})
    sdk = Comparison(work / "descriptor.json")
    sdk.validate_build()
    with open(os.environ["GITHUB_ENV"], "a") as env:
        for name, value in sdk.build_environment().items():
            if "\n" in value:
                raise RuntimeError("invalid workflow environment path")
            env.write(name + "=" + value + "\n")
        env.write("MIXYARD_COMPARISON_SDK=" + str(work / "descriptor.json") + "\n")


class Comparison:
    def __init__(self, descriptor):
        self.path = Path(descriptor).resolve(strict=True)
        self.data = json.loads(read_bounded(self.path))
        required = {"schema", "scope", "source", "prefix", "prefixManifestSha256", "sourceInputsSha256", "projectProvenanceSha256", "buildOptions", "tools"}
        if set(self.data) != required or self.data["schema"] != 1 or self.data["scope"] != "hosted-only-fixed-sdk-comparison" or self.data["source"] != SOURCE:
            raise RuntimeError("unknown or unpinned comparison descriptor")
        work = self.path.parent
        self.prefix = Path(self.data["prefix"]).resolve(strict=True)
        runner = Path(os.environ["RUNNER_TEMP"]).resolve(strict=True)
        if not work.is_relative_to(runner) or work == runner or self.prefix != work / "prefix" or work.stat().st_uid != os.getuid() or work.stat().st_mode & 0o077:
            raise RuntimeError("comparison prefix/root ownership mismatch")
        if digest(work / "prefix-manifest.json") != self.data["prefixManifestSha256"] or digest(work / "source-inputs.json") != self.data["sourceInputsSha256"]:
            raise RuntimeError("comparison input manifest digest mismatch")
        if digest(runner / "fixed-sdk-source-provenance.json") != self.data["projectProvenanceSha256"]:
            raise RuntimeError("project provenance changed")
        self.files = json.loads(read_bounded(work / "prefix-manifest.json"))
        if manifest(self.prefix) != self.files:
            raise RuntimeError("comparison installed files changed")
        self.snapshots = []
        self.graph = None

    def tool(self, name):
        if name not in ("pipewire", "pw-dump", "pw-cli"):
            raise RuntimeError("unknown comparison executable")
        file = self.prefix / "bin" / name
        if not file.resolve(strict=True).is_relative_to(self.prefix):
            raise RuntimeError("comparison executable outside prefix")
        return str(file)

    def build_environment(self):
        return {"PKG_CONFIG_PATH": str(self.prefix / "lib/pkgconfig"), "LD_LIBRARY_PATH": str(self.prefix / "lib"),
                "PIPEWIRE_MODULE_DIR": str(self.prefix / "lib/pipewire-0.3"), "SPA_PLUGIN_DIR": str(self.prefix / "lib/spa-0.2")}

    def validate_build(self):
        env = dict(os.environ, **self.build_environment())
        for package in ("libpipewire-0.3", "libspa-0.2"):
            actual = subprocess.check_output(["pkg-config", "--variable=prefix", package], env=env, text=True).strip()
            if Path(actual).resolve() != self.prefix:
                raise RuntimeError("pkg-config selected a different SDK")
        if subprocess.check_output(["pkg-config", "--modversion", "libpipewire-0.3"], env=env, text=True).strip() != VERSION:
            raise RuntimeError("compile SDK version mismatch")

    def verify_api(self, path):
        report = json.loads(read_bounded(path, 16384))
        library = (self.prefix / "lib/libpipewire-0.3.so.0").resolve(strict=True).relative_to(self.prefix).as_posix()
        if (report.get("publicMappingReadback") is not True or report.get("ownedAllocationBytes") != 32768 or
                report.get("headerVersion") != VERSION or report.get("runtimeVersion") != VERSION or
                report.get("resolvedApiLibraryBuildId") != self.files["files"][library]["buildId"] or
                report.get("pageSize") != 4096 or report.get("observed4224ShapeIncluded") is not True or
                report.get("allCasesPassed") is not True or len(report.get("cases", [])) != 8):
            raise RuntimeError("fixed SDK did not pass the identical eight API readbacks/identity")
        expected = ["zero-fresh", "page-offset-fresh", "page-offset-cover-zero", "nonzero-origin-fresh",
                    "nonzero-origin-same-page", "nonzero-origin-later-page", "observed-4224-fresh", "observed-4224-cover"]
        shapes = [(0, 0, 0), (4224, 0, 0), (4224, 0, 16384), (12416, 0, 0),
                  (8320, 8192, 16384), (12416, 8192, 16384), (4224, 0, 0), (4224, 0, 16384)]
        if ([item.get("name") for item in report["cases"]] != expected or
                any(item.get("requestedBytes") != 32 or
                    tuple(item.get(key) for key in ("requestedOffset", "coverOrigin", "coverBytes")) != shape
                    for item, shape in zip(report["cases"], shapes)) or any(
            any(item.get(key) is not True for key in ("preadComplete", "returnedMetadataMatches", "withinOwnedMapping", "readbackMatchesPread", "cleanupComplete")) for item in report["cases"])):
            raise RuntimeError("incomplete fixed SDK readback matrix")

    def summary(self):
        return {"scope": "hosted-only-comparison-observation-points-not-lifetime-closure", "source": SOURCE,
                "prefixManifestSha256": self.data["prefixManifestSha256"], "sourceInputsSha256": self.data["sourceInputsSha256"],
                "projectProvenanceSha256": self.data["projectProvenanceSha256"], "snapshots": self.snapshots, "shortLivedObjects": {"pw-dump": "unknown-not-observed", "pw-cli": "unknown-not-observed",
                "standaloneSpaProbe": "unknown-not-observed", "mappingApi": "resolved-build-id-in-separate-readback-report"}}

    def observe_owned(self, process, role, point):
        """Only the passed owned Popen PID, off RT; never enumerate other PIDs."""
        entry = {"scope": "declared-point-only", "role": role, "point": point, "status": "unknown",
                 "observerClock": "CLOCK_MONOTONIC", "beforeMonotonicNs": time.monotonic_ns(), "afterMonotonicNs": None}
        self.snapshots.append(entry)
        try:
            if role not in ("daemon", "graph") or process.poll() is not None:
                raise RuntimeError("owned comparison process unavailable")
            proc = Path("/proc") / str(process.pid)
            before = read_bounded(proc / "stat", 4096).rsplit(b")", 1)[1].split()[19]
            executable = Path(os.readlink(proc / "exe")).resolve(strict=True)
            expected = Path(self.tool("pipewire")) if role == "daemon" else self.graph
            if expected is None or executable != expected.resolve(strict=True):
                raise RuntimeError("owned comparison executable mismatch")
            objects = self.objects_from_maps(read_bounded(proc / "maps", 1024 * 1024))
            after = read_bounded(proc / "stat", 4096).rsplit(b")", 1)[1].split()[19]
            if before != after or process.poll() is not None or Path(os.readlink(proc / "exe")).resolve() != executable:
                raise RuntimeError("owned process changed during object observation")
            required = {"lib/libpipewire-0.3.so.0", "lib/spa-0.2/support/libspa-support.so"}
            if role == "daemon":
                required |= {"lib/pipewire-0.3/libpipewire-module-" + name + ".so" for name in MODULE_NAMES}
            else:
                required.add("lib/spa-0.2/audiomixer/libspa-audiomixer.so")
            required = {(self.prefix / name).resolve().relative_to(self.prefix).as_posix() for name in required}
            if not required <= set(objects):
                raise RuntimeError("critical SDK binding not actually observed at declared point")
            entry.update(status="observed-matching-prefix", executableSha256=digest(executable), objects=objects)
        except Exception as error:
            entry["reason"] = str(error)[:256] if isinstance(error, RuntimeError) else type(error).__name__ + ": observation unavailable"
            raise
        finally:
            entry["afterMonotonicNs"] = time.monotonic_ns()

    def objects_from_maps(self, raw):
        lines = raw.decode("utf-8", errors="strict").splitlines()
        if not raw.endswith(b"\n") or len(lines) > 4096:
            raise RuntimeError("incomplete or oversized owned object observation")
        result = {}
        deadline = time.monotonic() + 2
        for line in lines:
            if time.monotonic() > deadline:
                raise RuntimeError("bounded object observation deadline exceeded")
            fields = line.split(maxsplit=5)
            if len(fields) < 6 or not fields[5].startswith("/"):
                continue
            name = fields[5]
            candidate = Path(name)
            if not (candidate.is_relative_to(self.prefix) or "libpipewire" in candidate.name or "libspa-" in candidate.name):
                continue
            if name.endswith(" (deleted)") or not candidate.is_relative_to(self.prefix) or not candidate.resolve(strict=True).is_relative_to(self.prefix):
                raise RuntimeError("observed SDK object outside immutable prefix")
            relative = candidate.resolve().relative_to(self.prefix).as_posix()
            if relative in result:
                continue
            expected = self.files["files"].get(relative)
            if not expected or not expected["buildId"] or digest(candidate) != expected["sha256"] or build_id(candidate) != expected["buildId"]:
                raise RuntimeError("observed SDK object identity mismatch")
            result[relative] = {"sha256": expected["sha256"], "buildId": expected["buildId"]}
            if len(result) > 128:
                raise RuntimeError("owned SDK object cardinality exceeded")
        return result


if __name__ == "__main__":
    require_hosted()
    if len(sys.argv) == 3 and sys.argv[1] == "prepare":
        prepare(Path(sys.argv[2]))
    elif len(sys.argv) == 3 and sys.argv[1] == "validate-build":
        Comparison(sys.argv[2]).validate_build()
    elif len(sys.argv) == 4 and sys.argv[1] == "verify-api":
        Comparison(sys.argv[2]).verify_api(sys.argv[3])
    else:
        raise SystemExit("explicit comparison prepare/validate-build/verify-api arguments required")
