#!/usr/bin/env python3
"""Build and run the Mojo evaluator; no CLI contract logic lives in Python.

Setup: uv venv target/mojo-env --python 3.12
       uv pip install --python target/mojo-env/bin/python --require-hashes \
           -r qualities/mojo-requirements.txt
Run: python3 qualities/scripts/cli_contract.py /path/to/axvern
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
MOJO_VERSION = '1.1.0'


def run(binary):
    compiler = os.environ.get('MOJO') or str(ROOT / 'target/mojo-env/bin/mojo')
    if not Path(compiler).is_file():
        compiler = shutil.which(compiler) or shutil.which('mojo')
    if not compiler:
        raise SystemExit(__doc__)
    compiler = str(Path(compiler).resolve())
    version = subprocess.check_output([compiler, '--version'], text=True).strip()
    if not version.startswith(f'Mojo {MOJO_VERSION} '):
        raise SystemExit(f'Expected Mojo {MOJO_VERSION}, got {version}; see qualities/mojo-requirements.txt')
    # Select the shared Python library explicitly, rather than accidentally
    # loading an old system Python into Mojo (notably Apple's Python 3.9).
    python = Path(compiler).parent / 'python'
    if not python.is_file():
        python = Path(sys.executable)
    config = json.loads(subprocess.check_output([str(python), '-c',
        'import json,sys,sysconfig; print(json.dumps(dict(version=list(sys.version_info[:2]), '
        'home=sys.base_prefix, library=str(__import__("pathlib").Path(sysconfig.get_config_var("LIBDIR")) '
        '/ sysconfig.get_config_var("LDLIBRARY")))))'], text=True))
    if config['version'] < [3, 10] or not Path(config['library']).is_file():
        raise SystemExit('Mojo process probes require Python >=3.10 with a shared library; use the documented environment setup')
    env = os.environ.copy()
    env.update(MOJO_PYTHON_LIBRARY=config['library'], PYTHONHOME=config['home'],
               AXVERN_CONTRACT_PYTHON=str(python))
    output = ROOT / 'target/cli-contract'
    evidence = {'compiler': version, 'python_version': config['version'], 'stages': {}}

    def timed(name, command):
        start = time.perf_counter()
        result = subprocess.run(command, cwd=ROOT, env=env)
        elapsed = time.perf_counter() - start
        evidence['stages'][name] = {'seconds': elapsed, 'exit_code': result.returncode}
        print(f'CLI contract {name}: {elapsed:.3f}s', flush=True)
        (ROOT / 'target/cli-contract-evidence.json').write_text(json.dumps(evidence, indent=2) + '\n')
        result.check_returncode()

    timed('compile', [compiler, 'build', str(ROOT / 'qualities/cli_contract/main.mojo'), '-o', str(output)])
    timed('self_test', [str(output), '--self-test', 'qualities/cli_contract/fixtures.json'])
    timed('probes', [str(output), str(Path(binary).resolve(strict=True))])


if __name__ == '__main__':
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    run(sys.argv[1])
