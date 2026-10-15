"""Data-driven regression tests, including malformed output and prompt hangs."""
from std.python import Python
from contract import validate_response, string_value
from json import Node, parse_json, require
from probes import decode_stream, invoke
from python_context import PythonContext


def test_fixtures(path: String) raises:
    var text: String
    with open(path, "r") as file:
        text = file.read()
    var fixtures = parse_json(text)
    var count = 0
    for index in fixtures.nodes[fixtures.root].children:
        var name = string_value(fixtures, fixtures.member(index, "name"))
        var stdout = string_value(fixtures, fixtures.member(index, "stdout"))
        var stderr = string_value(fixtures, fixtures.member(index, "stderr"))
        var mode = string_value(fixtures, fixtures.member(index, "mode"))
        var rule = string_value(fixtures, fixtures.member(index, "rule"))
        var valid_index = fixtures.member(index, "valid")
        require(
            fixtures.nodes[valid_index].kind == Node.BOOL,
            "invalid fixture expectation",
        )
        var expected_valid = fixtures.nodes[valid_index].text == "true"
        var exit_index = fixtures.member(index, "exit_code")
        var exit_code = Int(fixtures.nodes[exit_index].text)
        var actual_valid = True
        var diagnostic = String()
        try:
            if mode == "json":
                _ = parse_json(stdout)
            else:
                require(mode == "response", "invalid fixture mode")
                _ = validate_response(stdout, stderr, exit_code)
        except error:
            actual_valid = False
            diagnostic = String(error)
        require(
            actual_valid == expected_valid,
            "fixture failed: " + name + " " + diagnostic,
        )
        if not expected_valid:
            require(
                rule in diagnostic, "wrong rule: " + name + " " + diagnostic
            )
        count += 1
    # Assert the decoded values, not just that parsing accepted the input.
    var unicode = parse_json('{"日本語":"\\ud83d\\udd25"}')
    require(
        string_value(unicode, unicode.member(unicode.root, "日本語")) == "🔥",
        "Unicode decoding failed",
    )
    print("Validator fixtures:", count, "passed")


def test_process_boundary() raises:
    var builtins = Python.import_module("builtins")
    var sys = Python.import_module("sys")
    var os = Python.import_module("os")
    var tempfile = Python.import_module("tempfile")
    var python = String(
        py=os.environ.get("AXVERN_CONTRACT_PYTHON", sys.executable)
    )
    var failed = False
    try:
        _ = decode_stream(builtins.bytes.fromhex("ff"))
    except error:
        failed = "CLI-JSON" in String(error)
    require(failed, "invalid UTF-8 accepted")
    with PythonContext(tempfile.TemporaryDirectory()) as root:
        failed = False
        try:
            var args: List[String] = ["-c", "input()"]
            _ = invoke(python, args, String(py=root), os.environ.copy(), 0.2)
        except error:
            failed = "CLI-INPUT" in String(error)
        require(failed, "open-stdin prompt was not detected")
        var args: List[String] = ["-c", "print('{}')"]
        var normal = invoke(python, args, String(py=root), os.environ.copy())
        require(
            normal.exit_code == 0
            and normal.stdout == "{}\n"
            and not normal.stderr,
            "normal subprocess failed",
        )
    print("Process boundary: UTF-8, prompt timeout and normal exit passed")
