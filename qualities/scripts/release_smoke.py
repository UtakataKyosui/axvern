#!/usr/bin/env python3
"""Build/install the public binary privately; record evidence, never publish."""
from datetime import datetime, timezone
import json
from pathlib import Path
import platform
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]


def run(args):
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


subprocess.run(["cargo", "build", "--release", "--locked", "--bin", "axvern"], cwd=ROOT, check=True)
with tempfile.TemporaryDirectory(prefix="axvern-install-") as directory:
    # Reuse the release build above instead of compiling again in a temporary target.
    subprocess.run(["cargo", "install", "--locked", "--path", "tools/cli", "--root", directory,
                    "--target-dir", str(ROOT / "target")], cwd=ROOT, check=True)
    binary = Path(directory) / "bin/axvern"
    version = run([str(binary), "--version"])
    assert version == "axvern 0.1.0", version
    help_text = run([str(binary), "--help"])
    for command in ["run", "status", "wait", "history", "doctor", "policy check"]:
        assert command in help_text, command
    # Only selected private configuration: no real user files or daemon.
    config = Path(directory) / "axvern.toml"
    config.write_text("schema_version=1\n")
    config.chmod(0o600)
    denied = subprocess.run([str(binary), "--config", str(config.resolve()), "run", "--format", "json", "--", "true"], cwd=ROOT, capture_output=True, text=True)
    assert denied.returncode == 9, denied
    assert not denied.stderr
    assert json.loads(denied.stdout)["error"]["code"] == "policy_denied"

evidence = {
    "schema_version": 1,
    "recorded_at": datetime.now(timezone.utc).isoformat(),
    "platform": platform.system(),
    "architecture": platform.machine(),
    "rust": run(["rustc", "--version"]),
    "pueue": run(["pueue", "--version"]),
    "binary": "axvern",
    "version": version,
    "checks": ["release_build", "private_cargo_install", "version", "help", "noninteractive_ask"],
    "published": False,
}
(ROOT / "target").mkdir(exist_ok=True)
(ROOT / "target/release-evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")
print(json.dumps(evidence))
