"""Strict JSON parser used at the untrusted CLI-output boundary.

Nodes live in a flat arena (Mojo does not allow recursive value types).
Reject duplicate decoded keys, non-JSON numbers and malformed Unicode escapes.
"""
from std.collections import List, Set
from std.collections.string import chr


def require(ok: Bool, message: String, rule: String = "AXV-SCHEMA") raises:
    if not ok:
        raise Error(rule + ": " + message)


@fieldwise_init
struct Node(Copyable, Movable):
    comptime NULL = 0
    comptime STRING = 1
    comptime INTEGER = 2
    comptime NUMBER = 3
    comptime BOOL = 4
    comptime OBJECT = 5
    comptime ARRAY = 6
    var kind: Int
    var text: String
    var keys: List[String]
    var children: List[Int]


@fieldwise_init
struct Document(Copyable, Movable):
    var nodes: List[Node]
    var root: Int

    def member(self, index: Int, key: String) raises -> Int:
        require(self.nodes[index].kind == Node.OBJECT, "expected object")
        for i in range(len(self.nodes[index].keys)):
            if self.nodes[index].keys[i] == key:
                return self.nodes[index].children[i]
        raise Error("AXV-SCHEMA: missing field: " + key)


struct Parser:
    var source: String
    var position: Int
    var nodes: List[Node]

    def __init__(out self, source: String):
        self.source = source
        self.position = 0
        self.nodes = List[Node]()

    def peek(self) -> Int:
        if self.position == self.source.byte_length():
            return -1
        return Int(self.source.as_bytes()[self.position])

    def whitespace(mut self):
        while (
            self.peek() == 32
            or self.peek() == 9
            or self.peek() == 10
            or self.peek() == 13
        ):
            self.position += 1

    def consume(mut self, byte: Int) raises:
        require(self.peek() == byte, "unexpected JSON token", "CLI-JSON")
        self.position += 1

    def hex4(mut self) raises -> Int:
        var value = 0
        for _ in range(4):
            var byte = self.peek()
            var digit = -1
            if 48 <= byte and byte <= 57:
                digit = byte - 48
            elif 65 <= byte and byte <= 70:
                digit = byte - 55
            elif 97 <= byte and byte <= 102:
                digit = byte - 87
            require(digit >= 0, "invalid Unicode escape", "CLI-JSON")
            value = value * 16 + digit
            self.position += 1
        return value

    def string(mut self) raises -> String:
        self.consume(34)
        var result = String()
        var start = self.position
        while self.peek() != 34:
            var byte = self.peek()
            require(
                byte >= 32, "unterminated string or control byte", "CLI-JSON"
            )
            if byte != 92:
                self.position += 1
                continue
            result += self.source[byte = start : self.position]
            self.position += 1
            byte = self.peek()
            self.position += 1
            if byte == 34 or byte == 92 or byte == 47:
                result += chr(byte)
            elif byte == 98:
                result += "\b"
            elif byte == 102:
                result += "\f"
            elif byte == 110:
                result += "\n"
            elif byte == 114:
                result += "\r"
            elif byte == 116:
                result += "\t"
            elif byte == 117:
                var point = self.hex4()
                if 0xD800 <= point and point <= 0xDBFF:
                    self.consume(92)
                    self.consume(117)
                    var low = self.hex4()
                    require(
                        0xDC00 <= low and low <= 0xDFFF,
                        "invalid surrogate pair",
                        "CLI-JSON",
                    )
                    point = 0x10000 + (point - 0xD800) * 1024 + low - 0xDC00
                require(
                    point < 0xD800 or point > 0xDFFF,
                    "unpaired surrogate",
                    "CLI-JSON",
                )
                result += chr(point)
            else:
                raise Error("CLI-JSON: invalid escape")
            start = self.position
        result += self.source[byte = start : self.position]
        self.position += 1
        return result

    def digits(mut self) raises:
        var start = self.position
        while 48 <= self.peek() and self.peek() <= 57:
            self.position += 1
        require(self.position > start, "expected digit", "CLI-JSON")

    def value(mut self, depth: Int) raises -> Int:
        # Probe resource budget, not a product response-depth contract.
        require(depth <= 128, "probe nesting budget exceeded", "CLI-JSON")
        self.whitespace()
        var node = Node(Node.NULL, "", List[String](), List[Int]())
        var byte = self.peek()
        if byte == 34:
            node.kind = Node.STRING
            node.text = self.string()
        elif byte == 123 or byte == 91:
            var object = byte == 123
            var seen_keys = Set[String]()
            node.kind = Node.OBJECT if object else Node.ARRAY
            var end = 125 if object else 93
            self.position += 1
            self.whitespace()
            if self.peek() != end:
                while True:
                    self.whitespace()
                    if object:
                        var key = self.string()
                        require(
                            key not in seen_keys, "duplicate key", "CLI-JSON"
                        )
                        seen_keys.add(key)
                        node.keys.append(key)
                        self.whitespace()
                        self.consume(58)
                    node.children.append(self.value(depth + 1))
                    self.whitespace()
                    if self.peek() != 44:
                        break
                    self.position += 1
            self.consume(end)
        elif byte == 110 or byte == 116 or byte == 102:
            var literal = "null"
            if byte != 110:
                literal = "true" if byte == 116 else "false"
                node.kind = Node.BOOL
            for expected in literal.as_bytes():
                self.consume(Int(expected))
            node.text = literal
        else:
            var start = self.position
            node.kind = Node.INTEGER
            if self.peek() == 45:
                self.position += 1
            if self.peek() == 48:
                self.position += 1
            else:
                self.digits()
            if self.peek() == 46:
                node.kind = Node.NUMBER
                self.position += 1
                self.digits()
            if self.peek() == 101 or self.peek() == 69:
                node.kind = Node.NUMBER
                self.position += 1
                if self.peek() == 43 or self.peek() == 45:
                    self.position += 1
                self.digits()
            node.text = String(self.source[byte = start : self.position])
        var index = len(self.nodes)
        self.nodes.append(node^)
        return index


def parse_json(source: String) raises -> Document:
    require("\u001b" not in source, "terminal escape on stdout", "CLI-COLOR")
    var parser = Parser(source)
    var root = parser.value(0)
    parser.whitespace()
    require(parser.peek() == -1, "expected one JSON document", "CLI-JSON")
    return Document(parser.nodes.copy(), root)
