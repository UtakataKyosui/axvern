"""Locate the shared CPython runtime for both Unix and macOS framework builds."""
import json
from pathlib import Path
import sys
import sysconfig


def shared_library(config, base_prefix):
    candidates = []
    libdir = config.get('LIBDIR')
    for name in ('LDLIBRARY', 'INSTSONAME'):
        if libdir and config.get(name):
            candidates.append(Path(libdir) / config[name])
    framework = config.get('PYTHONFRAMEWORK')
    if framework:
        # Framework CPython installs the loadable binary inside its versioned
        # base prefix, rather than LIBDIR/LDLIBRARY as on Linux and uv builds.
        candidates.append(Path(base_prefix) / framework)
    for candidate in candidates:
        if candidate.is_file():
            return str(candidate.resolve())
    raise RuntimeError('No shared Python library found in: ' + ', '.join(map(str, candidates)))


if __name__ == '__main__':
    print(json.dumps(dict(version=list(sys.version_info[:2]), home=sys.base_prefix,
                          library=shared_library(sysconfig.get_config_vars(), sys.base_prefix))))
