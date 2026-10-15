#!/usr/bin/env python3
"""Test-only child: inspect argv rather than relying on echo/shell interpretation."""
import json
import sys
import time

mode, *args = sys.argv[1:]
if mode == "argv":
    print(json.dumps(args, ensure_ascii=False))
elif mode == "streams":
    for i in range(3):
        print(f"out-{i}", flush=True)
        print(f"err-{i}", file=sys.stderr, flush=True)
elif mode == "large":
    for i in range(20000):
        print(f"{i:05d}:" + "x" * 100)
elif mode == "sleep":
    print("started", flush=True)
    time.sleep(float(args[0]))
    print("finished", flush=True)
elif mode == "fail":
    print("intentional failure", file=sys.stderr)
    sys.exit(23)
else:
    raise SystemExit("unknown probe mode")
