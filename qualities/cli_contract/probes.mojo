"""Daemon-free CLI probes. Only OS/process plumbing uses Python's stdlib.

Mojo's subprocess.run trims stdout and exposes neither stderr nor exit status;
use subprocess.Popen through the supported bridge to preserve the wire data.
No Python module implements parsing, schema validation or case evaluation.
"""
from std.python import Python, PythonObject
from std.collections import Optional
from contract import ErrorCode, string_value, validate_response
from json import Node, parse_json, require
from python_context import PythonContext


@fieldwise_init
struct Captured(Copyable, Movable):
    var stdout: String
    var stderr: String
    var exit_code: Int


def decode_stream(data: PythonObject) raises -> String:
    try:
        return String(py=data.decode("utf-8", errors="strict"))
    except:
        raise Error("CLI-JSON: expected UTF-8 output")


def invoke(
    binary: String,
    args: List[String],
    root: String,
    env: PythonObject,
    timeout: Float64 = 10.0,
) raises -> Captured:
    var sp = Python.import_module("subprocess")
    var tempfile = Python.import_module("tempfile")
    var builtins = Python.import_module("builtins")
    var command = builtins.list()
    command.append(binary)
    for arg in args:
        command.append(arg)
    # Files prevent a noisy child from exhausting validator memory. stdin stays
    # open until wait completes, so prompts cannot be hidden by an immediate EOF.
    with PythonContext(tempfile.TemporaryFile()) as stdout:
        with PythonContext(tempfile.TemporaryFile()) as stderr:
            with PythonContext(
                sp.Popen(
                    command,
                    cwd=root,
                    env=env,
                    stdin=sp.PIPE,
                    stdout=stdout,
                    stderr=stderr,
                )
            ) as process:
                var code: Int
                try:
                    code = Int(py=process.wait(timeout=timeout))
                except:
                    process.kill()
                    process.wait()
                    raise Error(
                        "CLI-INPUT: invocation blocked or exceeded probe"
                        " deadline"
                    )
                require(
                    Int(py=stdout.tell()) <= 1048576
                    and Int(py=stderr.tell()) <= 1048576,
                    "probe capture exceeds 1MiB",
                    "CLI-JSON",
                )
                stdout.seek(0)
                stderr.seek(0)
                return Captured(
                    decode_stream(stdout.read()),
                    decode_stream(stderr.read()),
                    code,
                )


@fieldwise_init
struct Case(Copyable, Movable):
    var name: String
    var args: List[String]
    var expected_exit: Int
    var expected_error: Optional[ErrorCode]


def load_cases(path: String) raises -> List[Case]:
    var text: String
    with open(path, "r") as file:
        text = file.read()
    var doc = parse_json(text)
    require(doc.nodes[doc.root].kind == Node.ARRAY, "cases must be an array")
    var cases = List[Case]()
    for index in doc.nodes[doc.root].children:
        var args_index = doc.member(index, "args")
        require(
            doc.nodes[args_index].kind == Node.ARRAY,
            "case args must be an array",
        )
        var args = List[String]()
        for arg in doc.nodes[args_index].children:
            args.append(string_value(doc, arg))
        var exit_index = doc.member(index, "expected_exit")
        require(
            doc.nodes[exit_index].kind == Node.INTEGER,
            "case exit must be integer",
        )
        var exit_code = Int(doc.nodes[exit_index].text)
        var error_index = doc.member(index, "expected_error")
        var error = Optional[ErrorCode]()
        if doc.nodes[error_index].kind != Node.NULL:
            var code = ErrorCode(string_value(doc, error_index))
            require(code.exit_code == exit_code, "inconsistent case error/exit")
            error = code^
        cases.append(
            Case(
                string_value(doc, doc.member(index, "name")),
                args^,
                exit_code,
                error^,
            )
        )
    return cases^


def evaluate_case(
    binary: String,
    config: String,
    root: String,
    env: PythonObject,
    probe_case: Case,
) raises:
    var args: List[String] = ["--config", config, "--format", "json"]
    for arg in probe_case.args:
        args.append(arg)
    try:
        var captured = invoke(binary, args, root, env)
        var response = validate_response(
            captured.stdout, captured.stderr, captured.exit_code
        )
        require(
            captured.exit_code == probe_case.expected_exit, "unexpected exit"
        )
        var error_code = String()
        if response.error:
            error_code = response.error.value().code.value
        var expected_error = String()
        if probe_case.expected_error:
            expected_error = probe_case.expected_error.value().value
        require(error_code == expected_error, "unexpected error")
        if captured.exit_code == 0:
            require(
                response.output
                and response.output.value().string_field("effect") == "ask",
                "expected Ask",
            )
    except error:
        raise Error(probe_case.name + ": " + String(error))


def probe(binary_path: String, cases_path: String) raises:
    var pathlib = Python.import_module("pathlib")
    var os = Python.import_module("os")
    var tempfile = Python.import_module("tempfile")
    var binary = String(py=pathlib.Path(binary_path).resolve(strict=True))
    with PythonContext(
        tempfile.TemporaryDirectory(prefix="axvern-contract-")
    ) as temporary:
        var root = String(py=pathlib.Path(temporary).resolve())
        var config = pathlib.Path(root) / "config.toml"
        config.write_text("schema_version=1\n", encoding="utf-8")
        config.chmod(0o600)
        var env = os.environ.copy()
        env.pop("AXVERN_CONFIG", None)
        env["HOME"] = root
        env["XDG_CONFIG_HOME"] = root
        var marker = pathlib.Path(root) / "helper-called"
        var helpers = pathlib.Path(root) / "bin"
        helpers.mkdir()
        var helper_names: List[String] = ["pueue", "less", "more"]
        for name in helper_names:
            var helper = helpers / name
            helper.write_text(
                '#!/bin/sh\ntouch "' + String(py=marker) + '"\nexit 99\n',
                encoding="utf-8",
            )
            helper.chmod(0o700)
        env["PATH"] = (
            String(py=helpers)
            + String(py=os.pathsep)
            + String(py=env.get("PATH", ""))
        )
        env["PAGER"] = String(py=helpers / "less")
        env["NO_COLOR"] = "1"
        env["TERM"] = "dumb"
        env["CLICOLOR_FORCE"] = "1"
        var flags: List[String] = ["--help", "--version"]
        var expected: List[String] = ["usage:", "axvern "]
        for i in range(len(flags)):
            var discovery_env = env.copy()
            discovery_env["AXVERN_CONFIG"] = root + "/missing.toml"
            var args: List[String] = [flags[i]]
            var result = invoke(binary, args, root, discovery_env)
            require(
                result.exit_code == 0
                and not result.stderr
                and expected[i] in result.stdout,
                "discovery failed",
                "CLI-HELP",
            )
            require(
                "\u001b" not in result.stdout,
                "colored discovery output",
                "CLI-COLOR",
            )
            print("PASS", flags[i])
        var cases = load_cases(cases_path)
        for probe_case in cases:
            evaluate_case(binary, String(py=config), root, env, probe_case)
            print("PASS", probe_case.name)
        var target = root + "/target-executed"
        config.write_text(
            "schema_version=1\n[[policy.rules]]\nid='probe'\neffect='allow'\ncommand='touch'\n",
            encoding="utf-8",
        )
        var args: List[String] = [
            "--config",
            String(py=config),
            "--format",
            "json",
            "policy",
            "check",
            "--",
            "touch",
            target,
        ]
        var result = invoke(binary, args, root, env)
        var response = validate_response(
            result.stdout, result.stderr, result.exit_code
        )
        require(
            result.exit_code == 0
            and response.output
            and response.output.value().string_field("effect") == "allow",
            "policy inspection failed",
            "CLI-VALIDATE",
        )
        require(
            not Bool(py=pathlib.Path(target).exists()),
            "policy inspection executed target",
            "CLI-VALIDATE",
        )
        print("PASS policy-no-execution")
        var secret = "AXVERN_CONTRACT_PRIVATE_SENTINEL"
        config.write_text(
            "schema_version=1\nunknown='" + secret + "'\n", encoding="utf-8"
        )
        args = [
            "--config",
            String(py=config),
            "--format",
            "json",
            "run",
            "--",
            "touch",
            target,
        ]
        result = invoke(binary, args, root, env)
        response = validate_response(
            result.stdout, result.stderr, result.exit_code
        )
        require(
            result.exit_code == 3
            and response.error
            and response.error.value().code.value == "invalid_config",
            "invalid config did not fail closed",
            "CLI-VALIDATE",
        )
        require(
            secret not in result.stdout and secret not in result.stderr,
            "secret echoed",
            "CLI-SECRETS",
        )
        require(
            not Bool(py=pathlib.Path(target).exists()),
            "invalid config executed target",
            "CLI-VALIDATE",
        )
        require(
            not Bool(py=marker.exists()),
            "backend or pager invoked",
            "CLI-VALIDATE",
        )
        print("PASS invalid-config-no-execution-or-leak")
        print("CLI contract:", len(cases) + 4, "probes passed (7 rule groups)")
