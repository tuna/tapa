"""Data structure to represent a blackbox file related to the design."""

import base64
import zlib

from tapa.graphir.types.commons import Model


class BlackBox(Model):
    """A definition of a blackbox file.

    Attributes:
        path (str): Relative path of the blackbox file.
        base64 (str): Base64 encoding of the content of the blackbox file.
    """

    path: str
    base64: bytes

    @staticmethod
    def from_binary(path: str, binary: bytes) -> "BlackBox":
        """Return a BlackBox from a binary file content.

        Examples:
            >>> m = BlackBox.from_binary("test.bin", b"test")
            >>> m.path
            'test.bin'
        """
        return BlackBox(path=path, base64=base64.b64encode(zlib.compress(binary)))

    def get_binary(self) -> bytes:
        """Return the binary representation of the file.

        Examples:
            >>> m = BlackBox.from_binary("test.bin", b"test")
            >>> m.get_binary()
            b'test'
        """
        return zlib.decompress(base64.b64decode(self.base64))
