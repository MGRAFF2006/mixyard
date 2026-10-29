"""Hosted-only production codec checks; never installs dependencies itself."""

import hashlib
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys


TARGET = Path(os.environ["CARGO_TARGET_DIR"])
EVIDENCE = TARGET / "evidence"
EVIDENCE.mkdir(parents=True, exist_ok=True)
MANIFEST = "network/codecs/Cargo.toml"


def run(label, args, timeout=180, env=None, rejection=None):
    result = subprocess.run(
        args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        text=True, errors="replace", timeout=timeout, check=False, env=env,
    )
    (EVIDENCE / f"{label}.txt").write_text(result.stdout, encoding="utf-8")
    print(result.stdout, flush=True)
    if rejection is None:
        result.check_returncode()
    elif result.returncode == 0 or rejection not in result.stdout:
        raise RuntimeError(f"{label}: missing expected build rejection {rejection}")
    return result.stdout


def digest(path):
    return f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path}\n"


def verify_loaded_bytes(output, selected):
    observed = {}
    for line in output.splitlines():
        if line.startswith("codec_runtime_symbol="):
            name, path = line.removeprefix("codec_runtime_symbol=").split("\t", 1)
            if name in observed:
                raise RuntimeError(f"duplicate runtime symbol {name}")
            observed[name] = Path(path)
    symbols = {"ZSTD_compress2": "ZSTD", "ZSTD_decompressDCtx": "ZSTD",
               "opus_encode_float": "OPUS", "opus_decode_float": "OPUS"}
    if observed.keys() != symbols.keys():
        raise RuntimeError(f"missing actual runtime symbol owners: {observed}")
    evidence = []
    for symbol, codec in symbols.items():
        actual, expected = observed[symbol], selected[codec]
        actual_hash = hashlib.sha256(actual.read_bytes()).hexdigest()
        expected_hash = hashlib.sha256(expected.read_bytes()).hexdigest()
        evidence.append(f"{symbol}: loaded={actual} sha256={actual_hash} "
                        f"selected={expected} sha256={expected_hash}\n")
        if actual_hash != expected_hash:
            raise RuntimeError(f"runtime bytes differ from selected {codec}: {actual}")
    return "".join(evidence)


def stage_runtime_decoy(source, owned):
    original = source.read_bytes()
    if sys.platform == "darwin":
        # Preserve the signed Mach-O layout. Appending data before codesign can
        # violate its signature-at-EOF guard. A fresh, explicitly owned fixture
        # identifier changes signature bytes while preserving code and ABI.
        owned.write_bytes(original)
        run(f"decoy-sign-{owned.name}", ["codesign", "--force", "--sign", "-",
                                        "--identifier", "mixyard-owned-codec-fixture", str(owned)])
    else:
        owned.write_bytes(original + b"\nMIXYARD_OWNED_DECOY\n")
    if owned.read_bytes() == original:
        raise RuntimeError("owned decoy bytes did not change")


def runtime_shadow_regression(exe, selected):
    """Owned ABI-compatible decoy with unchanged version, changed file bytes."""
    decoy = TARGET / "runtime-decoy"
    decoy.mkdir(exist_ok=True)
    decoy_exe = decoy / exe.name
    shutil.copy2(exe, decoy_exe)
    for codec, source in selected.items():
        # Preserve all installed SONAME/install-name aliases, not only the .so
        # development filename. They all contain the same deliberately modified
        # library. No system/package files or original runtime copies are edited.
        directory = (Path(os.environ["CODEC_DLL_DIR"]) if os.name == "nt"
                     else Path(os.environ[f"MIXYARD_{codec}_LIB_DIR"]))
        matches = list(directory.glob(f"*{codec.lower()}*.dll" if os.name == "nt"
                                      else f"*{codec.lower()}*.{'dylib' if sys.platform == 'darwin' else 'so*'}"))
        if not matches:
            raise RuntimeError(f"no runtime aliases for {codec}")
        for alias in matches:
            owned = decoy / alias.name
            stage_runtime_decoy(source, owned)
    env = dict(os.environ)
    loader = ("PATH" if os.name == "nt" else
              "DYLD_LIBRARY_PATH" if sys.platform == "darwin" else "LD_LIBRARY_PATH")
    env[loader] = str(decoy) + os.pathsep + env.get(loader, "")
    # The actual production example must still run all codec modes; merely
    # failing to load a corrupt/invalidly signed decoy does not pass this test.
    output = run("runtime-decoy-loopback", [str(decoy_exe)], timeout=60, env=env)
    try:
        verify_loaded_bytes(output, selected)
    except RuntimeError as error:
        if "runtime bytes differ from selected" not in str(error):
            raise
        (EVIDENCE / "runtime-decoy-rejected.txt").write_text(str(error), encoding="utf-8")
    else:
        raise RuntimeError("loader shadow decoy was not detected through production entry point")


def main():
    if shutil.disk_usage(TARGET).free < 10 * 2**30:
        raise RuntimeError("Less than 10 GiB free before codec qualification")
    revision = run("checkout", ["git", "rev-parse", "HEAD"]).strip()
    run("rust", ["rustc", "-vV"])
    kind = os.environ["MIXYARD_ZSTD_LINK_KIND"]
    assert kind == os.environ["MIXYARD_OPUS_LINK_KIND"]
    # Reapply here: macOS can strip DYLD_* while launching a protected shell.
    # Child codec binaries must see only the explicitly selected library dirs.
    if kind == "dylib" and os.name != "nt":
        loader = "DYLD_LIBRARY_PATH" if sys.platform == "darwin" else "LD_LIBRARY_PATH"
        os.environ[loader] = os.pathsep.join(
            os.environ[f"MIXYARD_{codec}_LIB_DIR"] for codec in ["ZSTD", "OPUS"])
    (EVIDENCE / "scope.txt").write_text(
        f"checkout={revision}\nsource={os.environ['CODEC_SOURCE_SHA']}\n"
        f"platform={platform.platform()} machine={platform.machine()}\n"
        f"link_kind={kind}\n"
        "Synthetic workers/TCP only; no physical/native-engine/browser proof.\n",
        encoding="utf-8",
    )
    keys = [f"MIXYARD_{codec}_{suffix}" for codec in ["ZSTD", "OPUS"]
            for suffix in ["LIB_DIR", "LIB_NAME", "LINK_KIND"]]
    keys += ["CODEC_DLL_DIR", "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH"]
    (EVIDENCE / "link-settings.txt").write_text(
        "".join(f"{key}={os.environ.get(key, '<default>')}\n" for key in keys), encoding="utf-8")
    # Single-feature builds catch accidental linkage to an unselected library.
    for feature in ["system-zstd", "system-opus"]:
        run(feature, ["cargo", "test", "--locked", "--offline", "--manifest-path",
                      MANIFEST, "--no-default-features", "--features", feature,
                      "--all-targets", "--", "--test-threads=1"])
    run("clippy", ["cargo", "clippy", "--locked", "--offline", "--manifest-path",
                   MANIFEST, "--all-features", "--all-targets"])
    run("tests", ["cargo", "test", "--locked", "--offline", "--manifest-path",
                  MANIFEST, "--all-features", "--all-targets", "--",
                  "--test-threads=1", "--nocapture"])
    run("build", ["cargo", "build", "--locked", "--offline", "--manifest-path",
                  MANIFEST, "--all-features", "--example", "loopback"])
    exe = TARGET / "debug" / "examples" / ("loopback.exe" if os.name == "nt" else "loopback")
    if os.name == "nt":
        vswhere = Path(os.environ["ProgramFiles(x86)"]) / "Microsoft Visual Studio/Installer/vswhere.exe"
        vs = run("visual-studio", [str(vswhere), "-latest", "-products", "*",
                                  "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                                  "-property", "installationPath"]).strip()
        dumpbins = sorted(Path(vs).glob("VC/Tools/MSVC/*/bin/Hostx64/x64/dumpbin.exe"))
        if not dumpbins:
            raise RuntimeError("MSVC dumpbin unavailable; cannot qualify PE imports")
        dumpbin = str(dumpbins[-1])
        links = run("linked-libraries", [dumpbin, "/dependents", str(exe)])
        if kind == "dylib":
            # Windows searches beside the executable first. Keep these owned
            # copies and their hashes with the actual fixture's import evidence.
            dlls = sorted(Path(os.environ["CODEC_DLL_DIR"]).glob("*.dll"))
            if not dlls:
                raise RuntimeError("Missing runtime DLLs from selected vcpkg triplet")
            for dll in dlls:
                shutil.copy2(dll, exe.parent / dll.name)
            (EVIDENCE / "runtime-dll-sha256.txt").write_text(
                "".join(digest(exe.parent / dll.name) for dll in dlls), encoding="utf-8")
    elif sys.platform == "darwin":
        links = run("linked-libraries", ["otool", "-L", str(exe)])
    else:
        links = run("linked-libraries", ["ldd", str(exe)])
    for library in ["zstd", "opus"]:
        present = library in links.lower()
        if present != (kind == "dylib"):
            raise RuntimeError(f"{library}: unexpected {kind} runtime linkage")
    # Preserve linker directives/archive paths as well as PE/Mach-O/ELF imports.
    build_outputs = sorted((TARGET / "debug" / "build").glob("mixyard-network-codecs-*/output"))
    (EVIDENCE / "cargo-link-directives.txt").write_text(
        "".join(f"{path}\n{path.read_text(encoding='utf-8')}\n" for path in build_outputs),
        encoding="utf-8",
    )
    hashes = [digest(exe)]
    if kind == "static":
        for codec in ["ZSTD", "OPUS"]:
            directory = Path(os.environ[f"MIXYARD_{codec}_LIB_DIR"])
            name = os.environ.get(f"MIXYARD_{codec}_LIB_NAME", codec.lower())
            archive = directory / (f"{name}.lib" if os.name == "nt" else f"lib{name}.a")
            hashes.append(digest(archive))
    (EVIDENCE / "sha256.txt").write_text("".join(hashes), encoding="utf-8")
    os.environ["MIXYARD_CODEC_RUNTIME_PROBE"] = "1"
    os.environ["MIXYARD_CODEC_RUNTIME_KIND"] = kind
    output = run("loopback", [str(exe)], timeout=60)
    if "linked_runtime_zstd=Some(" not in output or 'linked_runtime_opus=Some("' not in output:
        raise RuntimeError("Full-feature loopback did not report both linked runtime versions")
    for marker in ["Raw rate=", "Zstd {", "rate=16000 channels=1 blocks=64",
                   "rate=48000 channels=2 blocks=64"]:
        if marker not in output:
            raise RuntimeError(f"Missing production loopback mode: {marker}")
    selected = {}
    for codec in ["ZSTD", "OPUS"]:
        if kind == "static":
            selected[codec] = exe
        elif os.name == "nt":
            selected[codec] = Path(os.environ["CODEC_DLL_DIR"]) / f"{codec.lower()}.dll"
        else:
            name = os.environ.get(f"MIXYARD_{codec}_LIB_NAME", codec.lower())
            suffix = "dylib" if sys.platform == "darwin" else "so"
            selected[codec] = Path(os.environ[f"MIXYARD_{codec}_LIB_DIR"]) / f"lib{name}.{suffix}"
    (EVIDENCE / "actual-runtime-sha256.txt").write_text(
        verify_loaded_bytes(output, selected), encoding="utf-8")
    if kind == "dylib":
        runtime_shadow_regression(exe, selected)
    # Exercise configuration rejection through Cargo's actual build script.
    # Missing selected files must not fall through to another installed library.
    empty = TARGET / "empty-library-directory"
    empty.mkdir(exist_ok=True)
    check = ["cargo", "check", "--locked", "--offline", "--manifest-path", MANIFEST]
    for codec in ["ZSTD", "OPUS"]:
        original = Path(os.environ[f"MIXYARD_{codec}_LIB_DIR"])
        name = os.environ.get(f"MIXYARD_{codec}_LIB_NAME", codec.lower())
        filename = (f"{name}.lib" if os.name == "nt" else f"lib{name}.a" if kind == "static"
                    else f"lib{name}.{'dylib' if sys.platform == 'darwin' else 'so'}")
        owned = TARGET / f"cached-selection-{codec}"
        owned.mkdir(exist_ok=True)
        selected_file = owned / filename
        shutil.copy2(original / filename, selected_file)
        settings = {**os.environ, f"MIXYARD_{codec}_LIB_DIR": str(owned)}
        command = check + ["--features", f"system-{codec.lower()}"]
        run(f"cached-selection-{codec}-accepted", command, env=settings)
        selected_file.unlink()
        # Same environment and source, previously successful cached build. Cargo
        # must rerun selected-file admission rather than reusing stale output.
        run(f"cached-selection-{codec}-removed", command, env=settings,
            rejection="is missing selected library")
        for suffix, value, message in [
            ("LINK_KIND", "auto", "must be dylib or static"),
            ("LIB_NAME", "../codec", "must be a bare library name"),
            ("LIB_DIR", "relative-directory", "must be an existing absolute directory"),
            ("LIB_DIR", str(empty), "is missing selected library"),
        ]:
            run(f"reject-{codec}-{suffix}-{len(value)}",
                check + ["--features", f"system-{codec.lower()}"],
                env={**os.environ, f"MIXYARD_{codec}_{suffix}": value}, rejection=message)
    run("raw-ignores-native-settings", check + ["--no-default-features"],
        env={**os.environ, "MIXYARD_ZSTD_LINK_KIND": "auto", "MIXYARD_OPUS_LINK_KIND": "auto"})
    print(f"Qualified synthetic production workers: {platform.system()} / {kind}")


if __name__ == "__main__":
    main()
