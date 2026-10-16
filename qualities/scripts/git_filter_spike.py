#!/usr/bin/env python3
"""Reproduce configured filter execution by the proposed #23 diff argv.

Only a temporary repository and fixture programs are used. A process filter that
fails at startup is sufficient to prove that Git launched the configured program;
this spike does not claim to implement the long-running filter protocol.
"""
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile


def main():
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env.update(GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1")
    with tempfile.TemporaryDirectory(prefix="axvern-git-filter-") as temporary:
        root = Path(temporary)

        def git(*args, check=True):
            return subprocess.run(
                ["git", *args], cwd=root, env=env, capture_output=True, check=check
            )

        git("init", "--quiet")
        (root / "tracked.txt").write_text("old\n")
        (root / ".gitattributes").write_text("tracked.txt filter=fixture\n")
        git("add", ".")
        # The index supplies the baseline; no commit or user identity is needed.
        (root / "tracked.txt").write_text("changed content\n")
        helper = root / "filter.py"
        marker = root / "invoked"
        helper.write_text(
            "import pathlib, sys\n"
            "pathlib.Path(sys.argv[1]).write_text(sys.argv[2])\n"
            "if sys.argv[2] == 'process': sys.exit(1)\n"
            "sys.stdout.buffer.write(sys.stdin.buffer.read())\n"
        )
        observations = []
        for mode in ["clean", "process"]:
            command = shlex.join([sys.executable, str(helper), str(marker), mode])
            git("config", f"filter.fixture.{mode}", command)
            git("config", "filter.fixture.required", "true")
            for output in ["raw", "numstat", "patch"]:
                marker.unlink(missing_ok=True)
                result = git(
                    "--no-pager", "-c", "color.ui=never", "diff",
                    "--no-ext-diff", "--no-textconv", "--no-color", "-M",
                    f"--{output}", "-z", check=False,
                )
                invoked = marker.exists() and marker.read_text() == mode
                assert invoked, f"fixture did not invoke {mode} for {output}"
                observations.append(dict(filter=mode, output=output,
                                         exit_code=result.returncode, invoked=invoked))
        print(json.dumps(observations, indent=2))


if __name__ == "__main__":
    main()
