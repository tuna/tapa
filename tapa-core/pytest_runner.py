# ruff: noqa: INP001  -- standalone py_binary entrypoint, not a package.
"""Hermetic pytest entrypoint for the `tapa_cli_golden_test` sh_test.

The sh_test driver shells out to this `py_binary` instead of `python3
-m pytest` so the parity gate works in Bazel sandboxes whose system
`python3` is missing the `pytest` module — the `py_binary` launcher
stages `pytest` (and its transitive deps) from `@tapa_deps` into the
runfiles tree, then exec's into `pytest.main` with whatever argv the
shell driver passes through.
"""

import sys

import pytest


def main() -> int:
    """Forward CLI argv to `pytest.main`.

    Argv[0] is dropped to match Python's `-m pytest` shape.
    """
    return pytest.main(sys.argv[1:])


if __name__ == "__main__":
    sys.exit(main())
