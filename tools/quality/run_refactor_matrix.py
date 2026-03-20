"""Run representative refactor regression suites."""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from dataclasses import dataclass

SUPPORTED_SUITES = ("compiler", "graphir", "cosim", "visualizer")


@dataclass(frozen=True)
class SuiteCommand:
    """A command in one suite."""

    required_tool: str
    argv: tuple[str, ...]


SUITE_COMMANDS: dict[str, tuple[SuiteCommand, ...]] = {
    "compiler": (
        SuiteCommand(
            required_tool="bazel",
            argv=("bazel", "test", "--test_output=errors", "//tests/apps/vadd:vadd"),
        ),
    ),
    "graphir": (
        SuiteCommand(
            required_tool="pytest",
            argv=(
                "pytest",
                "-q",
                "tapa/graphir_conversion/leaf_task_conversion_test.py",
                "tapa/graphir_conversion/slot_task_conversion_test.py",
                "tapa/graphir_conversion/top_task_conversion_test.py",
            ),
        ),
    ),
    "cosim": (
        SuiteCommand(
            required_tool="bazel",
            argv=(
                "bazel",
                "test",
                "--test_output=errors",
                "//tests/apps/vadd:vadd-verilator-zipsim",
            ),
        ),
    ),
    "visualizer": (
        SuiteCommand(
            required_tool="pnpm",
            argv=("pnpm", "--dir", "tapa-visualizer", "lint", "src"),
        ),
        SuiteCommand(
            required_tool="pnpm",
            argv=("pnpm", "--dir", "tapa-visualizer", "build"),
        ),
    ),
}


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--suite",
        action="append",
        choices=SUPPORTED_SUITES,
        required=True,
        help="Suite to execute. Repeat for multiple suites.",
    )
    parser.add_argument(
        "--allow-missing-tools",
        action="store_true",
        help="Skip a suite command if its required tool is missing.",
    )
    return parser.parse_args()


def _run_command(command: SuiteCommand, *, allow_missing_tools: bool) -> bool:
    if shutil.which(command.required_tool) is None:
        msg = f"Required tool '{command.required_tool}' not found"
        if allow_missing_tools:
            sys.stderr.write(
                f"SKIP: {msg} for command: {' '.join(command.argv)}\n",
            )
            return True
        sys.stderr.write(f"ERROR: {msg}\n")
        return False
    sys.stderr.write(f"RUN: {' '.join(command.argv)}\n")
    result = subprocess.run(command.argv, check=False)
    return result.returncode == 0


def main() -> int:
    args = _parse_args()
    ok = True
    for suite in args.suite:
        sys.stderr.write(f"=== Suite: {suite} ===\n")
        for command in SUITE_COMMANDS[suite]:
            if not _run_command(command, allow_missing_tools=args.allow_missing_tools):
                ok = False
                sys.stderr.write(
                    f"FAIL: suite={suite} command={' '.join(command.argv)}\n",
                )
                break
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
