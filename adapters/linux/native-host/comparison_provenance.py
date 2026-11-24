"""Exact labelled-PR test-merge provenance; same actual-object rules as #105."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

LABEL = "qualify-pipewire-1.4.8"
REPOSITORY = "MGRAFF2006/mixyard"
PATHS = ("adapters/linux/native-host", "adapters/engine", "src", "Cargo.toml", "Cargo.lock",
         ".github/workflows/linux-native-host.yml")


def eligible(event_name, event):
    return event_name == "pull_request" and event.get("action") == "labeled" and event.get("label", {}).get("name") == LABEL


def git(*args, cwd=None):
    env = dict(os.environ, GIT_NO_REPLACE_OBJECTS="1", GIT_GRAFT_FILE=os.devnull)
    result = subprocess.run(["git", "-c", "core.commitGraph=false", "-c", "advice.graftFileDeprecated=false", *args],
                            cwd=cwd, env=env, capture_output=True)
    if result.returncode or result.stderr:
        raise RuntimeError("actual-object Git query failed: " + args[0])
    return result.stdout


def commit_chain(source, declared, target, cwd=None):
    for value in (source, declared):
        if not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{40}", value):
            raise RuntimeError("event source/base must be exact full SHAs")
    if target != "refs/remotes/origin/main":
        raise RuntimeError("approved target must be the independently fetched main ref")
    resolve = lambda value: git("rev-parse", "--verify", "--end-of-options", value + "^{commit}", cwd=cwd).decode().strip()
    executed, approved = resolve("HEAD"), resolve(target)
    if git("rev-parse", "--is-shallow-repository", cwd=cwd).strip() != b"false":
        raise RuntimeError("complete ancestry required")
    git("rev-list", "--quiet", executed, source, declared, approved, "--", cwd=cwd)
    parents = git("show", "-s", "--format=%P", executed, cwd=cwd).decode().split()
    if len(parents) != 2 or parents[1] != source:
        raise RuntimeError("comparison must execute exact [actual base, declared source] test merge")
    git("merge-base", "--is-ancestor", declared, parents[0], cwd=cwd)
    git("merge-base", "--is-ancestor", parents[0], approved, cwd=cwd)
    return {"declaredHead": source, "declaredBase": declared, "approvedTarget": approved,
            "approvedTargetRef": target, "executedMerge": executed, "orderedParents": parents,
            "executedTree": git("rev-parse", executed + "^{tree}", cwd=cwd).decode().strip(),
            "sourceTree": git("rev-parse", source + "^{tree}", cwd=cwd).decode().strip()}


def capture(event_path, output):
    event = json.loads(Path(event_path).read_bytes())
    result = {"scope": "label-is-run-selection-only-no-approval-or-audio-authority", "event": "pull_request",
              "action": event.get("action"), "label": LABEL, "valid": False}
    try:
        if not eligible(os.environ.get("GITHUB_EVENT_NAME"), event):
            raise RuntimeError("not the exact comparison label activity")
        pr = event["pull_request"]
        if event["repository"]["full_name"] != REPOSITORY or pr["head"]["repo"]["full_name"] != REPOSITORY or pr["base"]["repo"]["full_name"] != REPOSITORY or pr["base"]["ref"] != "main":
            raise RuntimeError("comparison repository/target mismatch")
        result.update(commit_chain(pr["head"]["sha"], pr["base"]["sha"], "refs/remotes/origin/main"))
        listing = git("ls-tree", "-r", "-z", result["executedMerge"], "--", *PATHS)
        working = {}
        for entry in listing.split(b"\0"):
            if not entry:
                continue
            metadata, raw = entry.split(b"\t", 1)
            _, kind, oid = metadata.split()
            name = raw.decode("utf-8", errors="strict")
            if kind != b"blob" or len(working) >= 8192:
                raise RuntimeError("invalid scoped source inventory")
            length = int(git("cat-file", "-s", oid.decode()))
            if length > 4 * 1024 * 1024:
                raise RuntimeError("scoped source blob too large")
            expected = hashlib.sha256(git("cat-file", "blob", oid.decode())).hexdigest()
            if hashlib.sha256(Path(name).read_bytes()).hexdigest() != expected:
                raise RuntimeError("working input differs from executed tree")
            working[name] = expected
        if git("status", "--porcelain=v1", "--untracked-files=all", "--", *PATHS):
            raise RuntimeError("scoped working inputs changed")
        result.update(valid=True, executedInputSha256=working)
    except BaseException as error:
        result["error"] = str(error) if isinstance(error, RuntimeError) else type(error).__name__
        raise
    finally:
        from comparison_sdk import write_json
        write_json(Path(output), result)


if __name__ == "__main__":
    capture(os.environ["GITHUB_EVENT_PATH"], sys.argv[1])
