#!/usr/bin/env python3
"""Moon builds the public binary first; pass its path across package boundaries."""
import json
import os
from pathlib import Path
import subprocess
from cli_contract import probe

metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"]))
binary = Path(metadata["target_directory"]) / "debug" / ("axvern.exe" if os.name == "nt" else "axvern")
if not binary.is_file():
    raise SystemExit("CLI missing: run moon run qualities:test (build dependency required)")
subprocess.run(["python3", "qualities/scripts/test_cli_contract.py"], check=True)
probe(binary)
env = os.environ.copy()
env["AXVERN_TEST_BIN"] = str(binary)
raise SystemExit(subprocess.call(["cargo", "test", "--workspace", "--locked"], env=env))
