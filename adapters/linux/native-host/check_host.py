#!/usr/bin/env python3
"""Run the real linked host oracle; no PipeWire API or SDK is loaded here."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

from request import request


def main():
    binary = Path(sys.argv[1]).resolve(strict=True)
    topology = {"clockId": 42, "sources": ["injected-a", "injected-b", "injected-c"],
                "outputs": ["injected-out-a", "injected-out-b", "injected-out-c"]}
    with tempfile.TemporaryDirectory(prefix="mixyard-injected-host-", dir=sys.argv[2]) as directory:
        for mode in ("normal", "output-failure", "tail-loss"):
            fixture = Path(directory) / (mode + ".json")
            fixture.write_text(json.dumps(request(topology, mode)))
            subprocess.run([str(binary), str(fixture), mode], check=True, timeout=10)


if __name__ == "__main__":
    main()
