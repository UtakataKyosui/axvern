"""Supply a real terminal approval without touching the user's terminal."""
import json
import os
import pty
import select
import subprocess
import sys
import time

master, slave = pty.openpty()
try:
    child = subprocess.Popen(sys.argv[2:], stdin=slave, stderr=slave, stdout=subprocess.PIPE)
    os.close(slave)
    slave = None
    transcript = b""
    deadline = time.monotonic() + 10
    while b"[y/N]" not in transcript:
        if time.monotonic() > deadline:
            child.kill()
            child.wait()
            raise RuntimeError("approval prompt missing")
        if select.select([master], [], [], 0.1)[0]:
            transcript += os.read(master, 4096)
    os.write(master, sys.argv[1].encode() + b"\n")
    stdout, _ = child.communicate(timeout=10)
    print(json.dumps({"exit": child.returncode, "stdout": stdout.decode(), "prompt": transcript.decode()}))
finally:
    os.close(master)
    if slave is not None:
        os.close(slave)
