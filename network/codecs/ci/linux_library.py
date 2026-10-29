"""Resolve an exact installed Debian development artifact; no metadata guessing."""

import argparse
from pathlib import Path
import subprocess
import sys


def resolve_directory(paths, artifact):
    if not artifact or Path(artifact).name != artifact:
        raise ValueError("a bare selected artifact filename is required")
    matches = {Path(line).parent.resolve() for line in paths
               if Path(line).name == artifact and Path(line).is_file()}
    if len(matches) != 1:
        raise ValueError(f"expected one installed {artifact}, found {sorted(map(str, matches))}")
    directory = matches.pop()
    if not (directory / artifact).is_file():
        raise ValueError(f"selected artifact missing from {directory}")
    return directory


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("package", choices=["libzstd-dev", "libopus-dev"])
    parser.add_argument("name", choices=["zstd", "opus"])
    parser.add_argument("kind", choices=["dylib", "static"])
    args = parser.parse_args()
    artifact = f"lib{args.name}.{'a' if args.kind == 'static' else 'so'}"
    result = subprocess.run(["dpkg-query", "-L", args.package], capture_output=True,
                            text=True, check=True, timeout=10)
    directory = resolve_directory(result.stdout.splitlines(), artifact)
    print(f"Selected installed {args.package} artifact: {directory / artifact}", file=sys.stderr)
    print(directory)


if __name__ == "__main__":
    main()
