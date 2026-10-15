#!/usr/bin/env python3
"""Schema-1 response validator and daemon-free public CLI contract probes.

Run: python3 qualities/scripts/cli_contract.py /path/to/axvern
Backend ownership and lifecycle remain covered by Rust integration tests.
"""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

# Selected interoperability rules, not full conformance to either specification.
# A2A-specific commands/envelopes/exit numbers do not apply to axvern.
RULES = {
    'CLI-JSON': ('Exactly one UTF-8 JSON document; no stdout prose or terminal escapes',
                 'https://github.com/a2aproject/a2a-cli/blob/main/specification/SPEC.md#113-machine-readable-output-json-and-its-streamed-form-jsonl'),
    'CLI-INPUT': ('Non-TTY invocation must finish without reading interactive input',
                  'https://clig.dev/#interactivity'),
    'CLI-HELP': ('Help and version are discoverable without configuration',
                 'https://clig.dev/#help'),
    'CLI-COLOR': ('Piped output is uncolored and does not invoke a pager',
                  'https://clig.dev/#output'),
    'CLI-VALIDATE': ('Reject invalid input before performing side effects',
                     'https://clig.dev/#robustness'),
    'CLI-SECRETS': ('Diagnostics must not echo secret configuration values',
                    'https://github.com/a2aproject/a2a-cli/blob/main/specification/SPEC.md#12-authentication'),
    'AXV-SCHEMA': ('Schema-1 fields, typed errors and exit mapping',
                   'sites/axvern-document-site/docs/en/cli.md'),
}

ERROR_EXITS = {
    'backend_failure': 1, 'invalid_input': 2, 'invalid_encoding': 2,
    'invalid_config': 3, 'permission_denied': 4, 'missing_tool': 5,
    'daemon_unavailable': 6, 'task_not_found': 7, 'expired': 8,
    'policy_denied': 9, 'partial_result': 11, 'output_missing': 11,
    'store_failure': 13, 'registration_unknown': 14, 'timeout': 124,
}
ID = re.compile(r'ex_[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}')


def require(condition, message, rule='AXV-SCHEMA'):
    if not condition:
        raise ValueError(f'{rule}: {message}')


def validate_machine_output(stdout):
    """General agent rule: one JSON document, not necessarily one physical line.

    Diagnostics on stderr are permitted by the external guidance. Duplicate keys,
    NaN and Infinity are rejected to avoid parser-dependent interpretations.
    Escaped user data such as a literal ESC inside a JSON string is preserved.
    """
    require(b'\x1b' not in stdout, 'terminal escape on stdout', 'CLI-COLOR')
    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, f'duplicate key: {key}', 'CLI-JSON')
            result[key] = value
        return result

    def invalid_constant(value):
        raise ValueError(f'CLI-JSON: invalid JSON constant: {value}')

    try:
        return json.loads(stdout.decode('utf-8'), object_pairs_hook=unique,
                          parse_constant=invalid_constant)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError('CLI-JSON: expected one UTF-8 JSON document') from error


def validate_response(stdout, stderr, exit_code):
    """Axvern profile: stronger stream/framing rules plus schema and exit mapping."""
    value = validate_machine_output(stdout)
    require(not stderr, 'unexpected JSON stderr')
    require(stdout.endswith(b'\n') and stdout.count(b'\n') == 1, 'expected one JSON line')
    require(isinstance(value, dict), 'response must be object')
    require({'schema_version', 'execution_id', 'state', 'exit_code', 'output', 'error'}
            <= value.keys(), 'missing field (use null, not omission)')
    require(type(value['schema_version']) is int and value['schema_version'] == 1, 'invalid schema')
    identifier = value['execution_id']
    require(identifier is None or isinstance(identifier, str) and ID.fullmatch(identifier), 'invalid ID')
    require(value['state'] in (None, 'queued', 'running', 'done', 'failed', 'partial'), 'invalid state')
    require(value['exit_code'] is None or type(value['exit_code']) is int, 'invalid child exit')
    require(value['output'] is None or isinstance(value['output'], dict), 'invalid output')
    error = value['error']
    if error is not None:
        require(isinstance(error, dict), 'invalid error')
        require(isinstance(error.get('message'), str), 'invalid message')
        code = error.get('code')
        require(isinstance(code, str) and code in ERROR_EXITS, 'unknown error code')
        require(exit_code == ERROR_EXITS[code], 'error/exit mismatch')
    else:
        require(exit_code in (0, 10, 12), 'unexpected success exit')
        if exit_code == 10:
            require(value['state'] == 'failed', 'exit 10 requires failed child')
        if exit_code == 12:
            require(value['output'] is not None and value['output'].get('level') == 'fail',
                    'exit 12 requires doctor Fail')
    return value


def invoke(binary, args, root, env, timeout=10):
    """Keep piped stdin OPEN: reading a prompt must time out, not merely see EOF.

    Capture to files so an erroneous noisy CLI cannot exhaust validator memory.
    The 1MiB capture budget is a probe safeguard, not axvern's response limit.
    """
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        with subprocess.Popen([binary, *args], cwd=root, env=env, stdin=subprocess.PIPE,
                              stdout=stdout, stderr=stderr) as process:
            try:
                code = process.wait(timeout=timeout)
            except subprocess.TimeoutExpired as error:
                process.kill()
                process.wait()
                raise ValueError('CLI-INPUT: invocation blocked or exceeded probe deadline') from error
            streams = []
            for stream in (stdout, stderr):
                require(stream.tell() <= 1048576, 'probe capture exceeds 1MiB', 'CLI-JSON')
                stream.seek(0)
                streams.append(stream.read())
            return subprocess.CompletedProcess(args, code, *streams)


def probe(binary):
    binary = str(Path(binary).resolve(strict=True))
    with tempfile.TemporaryDirectory(prefix='axvern-contract-') as temporary:
        root = Path(temporary).resolve()
        config = root / 'config.toml'
        config.write_text('schema_version=1\n', encoding='utf-8')
        config.chmod(0o600)
        env = os.environ.copy()
        env.pop('AXVERN_CONFIG', None)
        env.update(HOME=str(root), XDG_CONFIG_HOME=str(root))
        # Trap helpers: a policy-only probe must never reach a backend or pager.
        marker = root / 'helper-called'
        helpers = root / 'bin'
        helpers.mkdir()
        for name in ('pueue', 'less', 'more'):
            helper = helpers / name
            helper.write_text(f'#!/bin/sh\ntouch "{marker}"\nexit 99\n', encoding='utf-8')
            helper.chmod(0o700)
        env.update(PATH=str(helpers) + os.pathsep + env.get('PATH', ''),
                   PAGER=str(helpers / 'less'), NO_COLOR='1', TERM='dumb', CLICOLOR_FORCE='1')
        for flag, expected in (('--help', 'usage:'), ('--version', 'axvern ')):
            discovery_env = dict(env, AXVERN_CONFIG=str(root / 'missing.toml'))
            result = invoke(binary, [flag], root, discovery_env)
            require(result.returncode == 0 and not result.stderr and
                    expected in result.stdout.decode('utf-8'), 'discovery failed', 'CLI-HELP')
            require(b'\x1b' not in result.stdout, 'colored discovery output', 'CLI-COLOR')
        cases = [
            ('policy', ['policy', 'check', '--', 'true'], 0, None),
            ('argv-boundary', ['policy', 'check', '--', 'true', '--format', 'json', ''], 0, None),
            ('ask', ['run', '--', 'true'], 9, 'policy_denied'),
            ('missing-target', ['run', '--'], 2, 'invalid_input'),
            ('unknown', ['history', '--unknown', '1'], 2, 'invalid_input'),
            ('duplicate', ['history', '--limit', '1', '--limit', '2'], 2, 'invalid_input'),
            ('exclusive', ['policy', 'check', '--cwd', '.', '--', 'true'], 2, 'invalid_input'),
        ]
        for limit in ('0', '1001', '-1', '1.5', '18446744073709551616'):
            cases.append((f'limit-{limit}', ['history', '--limit', limit], 2, 'invalid_input'))
        for duration in ('0s', '1', '-1s', '1.5s', '1441m', '18446744073709551616ms'):
            cases.append((f'timeout-{duration}', ['wait', 'ex_550e8400-e29b-41d4-a716-446655440000',
                                                '--timeout', duration], 2, 'invalid_input'))
        # Accepted boundaries reach backend configuration validation (not parsing failure).
        for limit in ('1', '1000'):
            cases.append((f'limit-valid-{limit}', ['history', '--limit', limit], 3, 'invalid_config'))
        for duration in ('1ms', '1440m'):
            cases.append((f'timeout-valid-{duration}', ['wait', 'ex_550e8400-e29b-41d4-a716-446655440000',
                         '--timeout', duration], 3, 'invalid_config'))
        for name, args, expected_exit, expected_error in cases:
            result = invoke(binary, ['--config', str(config), '--format', 'json', *args], root, env)
            try:
                value = validate_response(result.stdout, result.stderr, result.returncode)
                require(result.returncode == expected_exit, 'unexpected exit')
                require((value['error'] or {}).get('code') == expected_error, 'unexpected error')
                if expected_exit == 0:
                    require(value['output'].get('effect') == 'ask', 'expected Ask')
            except ValueError as error:
                raise ValueError(f'{name}: {error}') from error
        # Policy check must inspect, never execute even an allowed target.
        target_marker = root / 'target-executed'
        config.write_text("schema_version=1\n[[policy.rules]]\nid='probe'\neffect='allow'\ncommand='touch'\n",
                          encoding='utf-8')
        result = invoke(binary, ['--config', str(config), '--format', 'json',
                                'policy', 'check', '--', 'touch', str(target_marker)], root, env)
        value = validate_response(result.stdout, result.stderr, result.returncode)
        require(value['output']['effect'] == 'allow' and result.returncode == 0,
                'policy inspection failed', 'CLI-VALIDATE')
        require(not target_marker.exists(), 'policy inspection executed target', 'CLI-VALIDATE')
        # Invalid config must fail closed, without leaking parser input.
        secret = 'AXVERN_CONTRACT_PRIVATE_SENTINEL'
        config.write_text(f"schema_version=1\nunknown='{secret}'\n", encoding='utf-8')
        result = invoke(binary, ['--config', str(config), '--format', 'json', 'run', '--',
                                'touch', str(target_marker)], root, env)
        value = validate_response(result.stdout, result.stderr, result.returncode)
        require(result.returncode == 3 and value['error']['code'] == 'invalid_config',
                'invalid config did not fail closed', 'CLI-VALIDATE')
        require(secret.encode() not in result.stdout + result.stderr, 'secret echoed', 'CLI-SECRETS')
        require(not target_marker.exists(), 'invalid config executed target', 'CLI-VALIDATE')
        require(not marker.exists(), 'backend or pager invoked', 'CLI-COLOR')
        print(f'CLI contract: {len(cases) + 4} probes passed ({len(RULES)} rule groups)')


if __name__ == '__main__':
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    probe(sys.argv[1])
