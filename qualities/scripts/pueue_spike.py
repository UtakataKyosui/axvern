#!/usr/bin/env python3
"""Pueue 4.0.4 black-box gate. Every client uses our explicit, private config.
Never falls back to a user daemon. Bounded commands, RAII-style teardown, no raw env snapshots.
"""
import argparse
import concurrent.futures
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

VERSION = "4.0.4"
ROOT = Path(__file__).resolve().parents[2]
PROBE = ROOT / "logics/tests/fixtures/pueue/argv_probe.py"


def run(argv, timeout=15, check=True, **kwargs):
    result = subprocess.run(argv, capture_output=True, timeout=timeout, **kwargs)
    if check and result.returncode:
        raise AssertionError(f"test command failed ({result.returncode}): {result.stderr.decode(errors='replace')[:1000]}")
    return result


class Queue:
    def __init__(self):
        # Short, canonical path prevents macOS unix socket length limits and /var symlinks.
        self.temp = tempfile.TemporaryDirectory(prefix="axv-", dir="/tmp")
        self.root = Path(self.temp.name).resolve()
        self.config = self.root / "config.yml"
        self.config.write_text("shared:\n" + "".join(
            f"  {key}: {json.dumps(str(value))}\n" for key, value in {
                "pueue_directory": self.root / "data",
                "runtime_directory": self.root / "runtime",
                "unix_socket_path": self.root / "socket",
                "pid_path": self.root / "daemon.pid",
                "alias_file": self.root / "aliases.yml",
            }.items()) + "  use_unix_socket: true\n" + "daemon:\n  shell_command: [sh, '-c', '{{ pueue_command_string }}']\n")
        self.config.chmod(0o600)
        self.daemon = None
        self.log = (self.root / "daemon.log").open("wb")
        try:
            self.start()
        except BaseException:
            self.close()
            raise

    def client(self, *args, **kwargs):
        return run(["pueue", "--config", str(self.config), "--color", "never", *map(str, args)], **kwargs)

    def start(self):
        self.daemon = subprocess.Popen(["pueued", "--config", str(self.config)], stdout=self.log, stderr=self.log)
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if self.daemon.poll() is not None:
                raise AssertionError("isolated daemon exited before readiness: " + (self.root / "daemon.log").read_text()[-2000:])
            if self.client("status", "--json", check=False).returncode == 0:
                return
            time.sleep(0.05)
        raise AssertionError("isolated daemon readiness timeout")

    def tasks(self):
        return json.loads(self.client("status", "--json").stdout)["tasks"]

    def wait_empty(self):
        deadline = time.monotonic() + 5
        while self.tasks():
            assert time.monotonic() < deadline, "queue did not become empty"
            time.sleep(0.02)

    def add(self, mode, *args):
        # Pueue escapes individual argv; no shell command interpolation in the harness.
        result = self.client("add", "--print-task-id", "--escape", "--", sys.executable, PROBE, mode, *args)
        raw = result.stdout.decode().strip()
        assert raw.isdecimal(), "print-task-id must return a numeric ID only"
        return raw

    def wait(self, task):
        return self.client("wait", "--quiet", task)

    def logs(self, task, *args):
        return json.loads(self.client("log", "--json", *args, task).stdout)

    def close(self):
        if self.daemon is not None and self.daemon.poll() is None:
            try:
                self.client("reset", check=False, timeout=5)
                self.wait_empty()
                self.client("shutdown", check=False, timeout=5)
                self.daemon.wait(timeout=5)
            except (subprocess.TimeoutExpired, AssertionError):
                self.daemon.kill()
                self.daemon.wait(timeout=5)
        self.log.close()
        self.temp.cleanup()


def output(logs, task):
    return logs[str(task)]["output"]


def spike():
    evidence = {"pueue_version": VERSION, "platform": sys.platform, "checks": {}}
    q = Queue()
    path = q.root
    try:
        assert q.tasks() == {}
        args = ["", "a b", "single'quote", 'double"quote', "line\nbreak", "$(touch INJECTED)", "`touch INJECTED`", "; touch INJECTED", "*?[a]", "\\", "日本語🦀", "--config", "-n"]
        task = q.add("argv", *args)
        q.wait(task)
        logs = q.logs(task, "--full")
        assert json.loads(output(logs, task)) == args
        assert not (ROOT / "INJECTED").exists()
        evidence["checks"]["argv"] = {"cases": args, "exact_roundtrip": True}
        evidence["status_task_keys"] = sorted(q.tasks()[task])
        # Only retain schema/status, never environment or local paths from Pueue task JSON.
        evidence["success_status"] = q.tasks()[task]["status"]
        assert evidence["success_status"]["Done"]["result"] == "Success"
        evidence["log_entry_keys"] = sorted(logs[task])
        task = q.add("streams")
        q.wait(task)
        stream = output(q.logs(task, "--full"), task)
        assert stream.splitlines() == [s for i in range(3) for s in (f"out-{i}", f"err-{i}")]
        evidence["checks"]["merged_stream"] = stream
        task = q.add("fail")
        wait = q.wait(task)
        evidence["checks"]["child_failure"] = {"wait_cli_exit": wait.returncode, "status": q.tasks()[task]["status"]}
        assert wait.returncode == 0
        assert q.tasks()[task]["status"]["Done"]["result"] == {"Failed": 23}
        task = q.add("sleep", "3")
        deadline = time.monotonic() + 5
        while "Running" not in q.tasks()[task]["status"]:
            assert time.monotonic() < deadline
            time.sleep(0.02)
        try:
            q.client("wait", "--quiet", task, timeout=0.1)
            raise AssertionError("wait unexpectedly completed")
        except subprocess.TimeoutExpired:
            pass
        assert "Running" in q.tasks()[task]["status"]
        deadline = time.monotonic() + 2
        while True:
            live = output(q.logs(task, "--full"), task)
            if live is not None and "started\n" in live:
                break
            assert time.monotonic() < deadline, "running log did not become available"
            time.sleep(0.02)
        assert "finished" not in live
        q.wait(task)
        assert "finished" in output(q.logs(task, "--full"), task)
        evidence["checks"]["timeout"] = {"client_timeout_task_continues": True, "running_log": live}
        task = q.add("sleep", "30")
        deadline = time.monotonic() + 5
        while "Running" not in q.tasks()[task]["status"]:
            assert time.monotonic() < deadline
            time.sleep(0.02)
        q.client("kill", task)
        q.wait(task)
        evidence["checks"]["killed"] = q.tasks()[task]["status"]
        assert evidence["checks"]["killed"]["Done"]["result"] == "Killed"
        task = q.add("large")
        q.wait(task)
        start = time.monotonic()
        full = output(q.logs(task, "--full"), task)
        elapsed = time.monotonic() - start
        tail = output(q.logs(task, "--lines", "5"), task)
        assert len(full.encode()) > 2_000_000 and len(tail.encode()) < 1000
        evidence["checks"]["large_log"] = {"full_bytes": len(full.encode()), "tail_bytes": len(tail.encode()), "full_seconds": round(elapsed, 4), "tail_lines": len(tail.splitlines())}
        before = q.tasks()
        q.client("shutdown")
        q.daemon.wait(timeout=5)
        assert q.client("status", "--json", check=False).returncode != 0
        q.start()
        assert q.tasks() == before
        evidence["checks"]["restart"] = {"completed_tasks_preserved": True, "daemon_unavailable_cli_nonzero": True}
        q.client("clean")
        q.wait_empty()
        missing = q.client("log", "--json", task, check=False)
        assert missing.returncode == 0 and json.loads(missing.stdout) == {}
        evidence["checks"]["clean"] = {"log_cli_exit": missing.returncode, "log_stdout": missing.stdout.decode()}
        new_id = q.add("argv", "after clean")
        q.wait(new_id)
        q.client("reset")
        q.wait_empty()
        reset_id = q.add("argv", "after reset")
        q.wait(reset_id)
        evidence["checks"]["ids"] = {"after_clean": int(new_id), "after_reset": int(reset_id), "id_reused_after_reset": int(reset_id) <= int(new_id)}
        assert new_id == reset_id == "0", "pinned Pueue ID reuse contract changed"
        # A finite-lived child avoids leaking work even if the daemon dies unexpectedly.
        task = q.add("sleep", "0.5")
        deadline = time.monotonic() + 5
        while "Running" not in q.tasks()[task]["status"]:
            assert time.monotonic() < deadline
            time.sleep(0.02)
        q.daemon.kill()
        crash_exit = q.daemon.wait(timeout=5)
        offline = q.client("status", "--json", check=False).returncode
        time.sleep(0.7)
        q.start()
        evidence["checks"]["daemon_crash"] = {"daemon_exit": crash_exit, "offline_cli_exit": offline, "restored_status": q.tasks()[task]["status"]}
        assert crash_exit != 0 and offline != 0
        assert evidence["checks"]["daemon_crash"]["restored_status"]["Done"]["result"] == "Killed"
        return evidence
    finally:
        q.close()
        assert not path.exists(), "temporary queue leaked"


def isolation_probe(_):
    q = Queue()
    path = q.root
    try:
        task = q.add("sleep", "0.2")
        assert list(q.tasks()) == [task]
        q.wait(task)
        return str(path)
    finally:
        q.close()
        assert not path.exists()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--record", type=Path)
    args = parser.parse_args()
    for tool in ("pueue", "pueued"):
        assert run([tool, "--version"]).stdout.decode().strip() == f"{tool} {VERSION}", f"requires {tool} {VERSION}"
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        paths = list(pool.map(isolation_probe, range(2)))
    assert len(set(paths)) == 2
    evidence = spike()
    evidence["checks"]["parallel_isolation_cleanup"] = True
    if args.record:
        args.record.parent.mkdir(parents=True, exist_ok=True)
        args.record.write_text(json.dumps(evidence, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(evidence, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
