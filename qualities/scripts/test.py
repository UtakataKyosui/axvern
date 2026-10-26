#!/usr/bin/env python3
"""Moon builds the public binary first; pass its path across package boundaries."""
import json
import os
from pathlib import Path
import subprocess
import sys
from cli_contract import run

metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"]))
binary = Path(metadata["target_directory"]) / "debug" / ("axvern.exe" if os.name == "nt" else "axvern")
if not binary.is_file():
    raise SystemExit("CLI missing: run moon run qualities:test (build dependency required)")
nextest = subprocess.run(["cargo", "nextest", "--version"], capture_output=True).returncode == 0
if not nextest and "AXVERN_REQUIRE_NEXTEST" in os.environ:
    raise SystemExit("cargo-nextest required: see qualities/index.md")
if "AXVERN_REQUIRE_DISCOVERY_TOOLS" in os.environ:
    subprocess.run([sys.executable, "qualities/scripts/discovery_tools.py"], check=True)
run(binary)
env = os.environ.copy()
env["AXVERN_TEST_BIN"] = str(binary)
if nextest:
    # nextest does not run doctests; run them separately to keep cargo test's coverage.
    codes = [subprocess.call(["cargo", "nextest", "run", "--workspace", "--locked"], env=env),
             subprocess.call(["cargo", "test", "--doc", "--workspace", "--locked"], env=env)]
    raise SystemExit(next((code for code in codes if code), 0))
raise SystemExit(subprocess.call(["cargo", "test", "--workspace", "--locked"], env=env))
