"""Convert untrusted JSON into typed schema-1 values before evaluating cases."""
from std.collections import Optional
from json import Document, Node, parse_json, require


struct ExecutionId(Copyable, Movable):
    var value: String

    def __init__(out self, value: String) raises:
        require(
            value.byte_length() == 39 and value.startswith("ex_"),
            "invalid execution ID",
        )
        for i in range(3, 39):
            var c = String(value[byte=i])
            if i == 11 or i == 16 or i == 21 or i == 26:
                require(c == "-", "invalid execution ID")
            else:
                require(c in "0123456789abcdef", "invalid execution ID")
        require(
            value[byte=17] == "4" and String(value[byte=22]) in "89ab",
            "invalid UUIDv4",
        )
        self.value = value


struct State(Copyable, Movable):
    var value: String

    def __init__(out self, value: String) raises:
        require(
            value == "queued"
            or value == "running"
            or value == "done"
            or value == "failed"
            or value == "partial",
            "invalid state",
        )
        self.value = value


struct ErrorCode(Copyable, Movable):
    var value: String
    var exit_code: Int

    def __init__(out self, value: String) raises:
        var names: List[String] = [
            "backend_failure",
            "invalid_input",
            "invalid_encoding",
            "invalid_config",
            "permission_denied",
            "missing_tool",
            "daemon_unavailable",
            "task_not_found",
            "expired",
            "policy_denied",
            "partial_result",
            "output_missing",
            "store_failure",
            "registration_unknown",
            "timeout",
        ]
        var exits: List[Int] = [
            1,
            2,
            2,
            3,
            4,
            5,
            6,
            7,
            8,
            9,
            11,
            11,
            13,
            14,
            124,
        ]
        for i in range(len(names)):
            if value == names[i]:
                self.value = value
                self.exit_code = exits[i]
                return
        raise Error("AXV-SCHEMA: unknown error code")


@fieldwise_init
struct ContractError(Copyable, Movable):
    var code: ErrorCode
    var message: String


@fieldwise_init
struct Output(Copyable, Movable):
    # Keep arbitrary command-specific output in a validated JSON document.
    var document: Document
    var index: Int

    def string_field(self, key: String) raises -> String:
        return string_value(
            self.document, self.document.member(self.index, key)
        )


@fieldwise_init
struct Response(Copyable, Movable):
    var schema_version: Int
    var execution_id: Optional[ExecutionId]
    var state: Optional[State]
    var exit_code: Optional[Int32]
    var output: Optional[Output]
    var error: Optional[ContractError]


def string_value(doc: Document, index: Int) raises -> String:
    require(doc.nodes[index].kind == Node.STRING, "expected string")
    return doc.nodes[index].text


def child_exit_value(doc: Document, index: Int) raises -> Int32:
    require(doc.nodes[index].kind == Node.INTEGER, "invalid child exit")
    var number: Int
    try:
        number = Int(doc.nodes[index].text)
    except:
        raise Error("AXV-SCHEMA: child exit outside i32")
    require(
        -2147483648 <= number and number <= 2147483647, "child exit outside i32"
    )
    return Int32(number)


def validate_response(
    stdout: String, stderr: String, exit_code: Int
) raises -> Response:
    var doc = parse_json(stdout)
    require(not stderr, "unexpected JSON stderr")
    require(
        stdout.endswith("\n") and stdout.count("\n") == 1,
        "expected one JSON line",
    )
    var root = doc.root
    var schema = doc.member(root, "schema_version")
    require(
        doc.nodes[schema].kind == Node.INTEGER
        and doc.nodes[schema].text == "1",
        "invalid schema",
    )
    var identifier = doc.member(root, "execution_id")
    var state = doc.member(root, "state")
    var child = doc.member(root, "exit_code")
    var output = doc.member(root, "output")
    var error = doc.member(root, "error")
    var result = Response(
        1,
        Optional[ExecutionId](),
        Optional[State](),
        Optional[Int32](),
        Optional[Output](),
        Optional[ContractError](),
    )
    if doc.nodes[identifier].kind != Node.NULL:
        result.execution_id = ExecutionId(string_value(doc, identifier))
    if doc.nodes[state].kind != Node.NULL:
        result.state = State(string_value(doc, state))
    if doc.nodes[child].kind != Node.NULL:
        result.exit_code = child_exit_value(doc, child)
    if doc.nodes[output].kind != Node.NULL:
        require(doc.nodes[output].kind == Node.OBJECT, "invalid output")
        result.output = Output(doc.copy(), output)
    if doc.nodes[error].kind != Node.NULL:
        var code = ErrorCode(string_value(doc, doc.member(error, "code")))
        var message = string_value(doc, doc.member(error, "message"))
        require(exit_code == code.exit_code, "error/exit mismatch")
        result.error = ContractError(code^, message)
    else:
        require(
            exit_code == 0 or exit_code == 10 or exit_code == 12,
            "unexpected success exit",
        )
        if exit_code == 10:
            require(
                result.state and result.state.value().value == "failed",
                "exit 10 requires failed child",
            )
        if exit_code == 12:
            require(
                result.output
                and result.output.value().string_field("level") == "fail",
                "exit 12 requires doctor Fail",
            )
    return result^
