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
    fallback_commands: tuple[SuiteCommand, ...] = ()


SUITE_COMMANDS: dict[str, tuple[SuiteCommand, ...]] = {
    "compiler": (
        SuiteCommand(
            required_tool="bazel",
            argv=("bazel", "build", "//tests/apps/vadd:vadd-xo"),
        ),
        SuiteCommand(
            required_tool="bazel",
            argv=("bazel", "build", "//tests/apps/vadd:vadd-zip"),
        ),
    ),
    "graphir": (
        SuiteCommand(
            required_tool="bazel",
            argv=(
                "bazel",
                "test",
                "--test_output=errors",
                "//tests/functional/graphir:vadd-xosim",
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
                "//tests/apps/vadd:vadd-zipsim",
            ),
        ),
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
            fallback_commands=(
                SuiteCommand(
                    required_tool="npm",
                    argv=(
                        "npm",
                        "--prefix",
                        "tapa-visualizer",
                        "run",
                        "lint",
                        "--",
                        "src",
                    ),
                ),
            ),
        ),
        SuiteCommand(
            required_tool="pnpm",
            argv=("pnpm", "--dir", "tapa-visualizer", "build"),
            fallback_commands=(
                SuiteCommand(
                    required_tool="npm",
                    argv=(
                        "npm",
                        "--prefix",
                        "tapa-visualizer",
                        "run",
                        "build",
                    ),
                ),
            ),
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


def _select_command(command: SuiteCommand) -> SuiteCommand | None:
    for candidate in (command, *command.fallback_commands):
        if shutil.which(candidate.required_tool) is not None:
            return candidate
    return None


def _run_command(command: SuiteCommand, *, allow_missing_tools: bool) -> bool:
    selected = _select_command(command)
    if selected is None:
        msg = " or ".join(
            f"'{c.required_tool}'" for c in (command, *command.fallback_commands)
        )
        if allow_missing_tools:
            sys.stderr.write(
                f"SKIP: Required tool(s) {msg} not found for command: "
                f"{' '.join(command.argv)}\n",
            )
            return True
        sys.stderr.write(f"ERROR: Required tool(s) {msg} not found\n")
        return False
    sys.stderr.write(f"RUN: {' '.join(selected.argv)}\n")
    return subprocess.run(selected.argv, check=False).returncode == 0


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
