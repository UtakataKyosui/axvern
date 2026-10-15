"""Exception-safe resource ownership for Python stdlib context managers."""
from std.python import PythonObject


@fieldwise_init
struct PythonContext:
    var value: PythonObject

    def __enter__(mut self) raises -> PythonObject:
        return self.value.__enter__()

    def __exit__(mut self) raises:
        # Always release the resource; never suppress a Mojo validation failure.
        self.value.__exit__(None, None, None)
