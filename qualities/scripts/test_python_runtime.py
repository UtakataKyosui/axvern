"""Exercise library discovery without depending on the host's Python layout."""
from pathlib import Path
import tempfile
import unittest
from python_runtime import shared_library


class RuntimeTests(unittest.TestCase):
    def test_unix_shared_library(self):
        with tempfile.TemporaryDirectory() as root:
            library = Path(root) / 'libpython3.12.so'
            library.touch()
            self.assertEqual(shared_library(dict(LIBDIR=root, LDLIBRARY=library.name), root), str(library.resolve()))

    def test_macos_framework_library(self):
        with tempfile.TemporaryDirectory() as root:
            prefix = Path(root) / 'Python.framework/Versions/3.12'
            prefix.mkdir(parents=True)
            library = prefix / 'Python'
            library.touch()
            self.assertEqual(shared_library(dict(LIBDIR=str(prefix / 'lib'), LDLIBRARY='Python',
                                                 PYTHONFRAMEWORK='Python'), prefix), str(library.resolve()))

    def test_missing_library_fails(self):
        with tempfile.TemporaryDirectory() as root:
            with self.assertRaisesRegex(RuntimeError, 'No shared Python library'):
                shared_library(dict(LIBDIR=root, LDLIBRARY='missing'), root)


if __name__ == '__main__':
    unittest.main()
