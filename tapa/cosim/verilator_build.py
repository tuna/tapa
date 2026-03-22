"""Build and launch helpers for Verilator cosimulation."""

from __future__ import annotations

import logging
import os
import shutil
import subprocess
import sys
from pathlib import Path

_logger = logging.getLogger().getChild(__name__)


def launch_verilator(config: dict, tb_output_dir: str) -> None:
    top_name: str = config["top_name"]
    _logger.info("Building Verilator simulation for %s", top_name)

    build_script = Path(tb_output_dir) / "build.sh"
    result = subprocess.run(
        [str(build_script)],
        check=False,
        cwd=tb_output_dir,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        _logger.error("Verilator build failed:\n%s\n%s", result.stdout, result.stderr)
        sys.exit(result.returncode)
    _logger.info("Verilator build succeeded")

    binary = Path(tb_output_dir) / f"obj_dir/V{top_name}"
    _logger.info("Running Verilator simulation")
    result = subprocess.run(
        [str(binary)],
        check=False,
        cwd=tb_output_dir,
        capture_output=True,
        text=True,
    )
    sys.stdout.write(result.stdout)
    sys.stderr.write(result.stderr)

    if result.returncode != 0:
        _logger.error("Verilator simulation failed with code %d", result.returncode)
        sys.exit(result.returncode)
    _logger.info("Verilator simulation finished successfully")


def generate_build_script(top_name: str) -> str:
    verilator_bin, verilator_root = _find_verilator()

    warn_flags = (
        "-Wno-fatal -Wno-PINMISSING -Wno-WIDTH"
        " -Wno-UNUSEDSIGNAL -Wno-UNDRIVEN -Wno-UNOPTFLAT"
        " -Wno-STMTDLY -Wno-CASEINCOMPLETE -Wno-SYMRSVDWORD"
        " -Wno-COMBDLY -Wno-TIMESCALEMOD -Wno-MULTIDRIVEN"
    )

    root_export = f'export VERILATOR_ROOT="{verilator_root}"' if verilator_root else ""

    return f"""\
#!/bin/bash
set -e
cd "$(dirname "$0")"

{root_export}

{verilator_bin} --cc --top-module {top_name} \\
  {warn_flags} \\
  --no-timing \\
  --exe tb.cpp dpi_support.cpp \\
  rtl/*.v 2>&1

make -C obj_dir -f V{top_name}.mk V{top_name} \\
  -j$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4) 2>&1
"""


def _find_verilator() -> tuple[str, str | None]:
    env_bin = os.environ.get("VERILATOR_BIN")
    if env_bin:
        verilator_bin = str(Path(env_bin).resolve())
        if not Path(verilator_bin).is_file():
            _logger.error("VERILATOR_BIN=%s does not exist", env_bin)
            sys.exit(1)
        verilator_root = _find_verilator_root(verilator_bin)
        _logger.info(
            "Using Bazel Verilator: %s (root: %s)", verilator_bin, verilator_root
        )
        return verilator_bin, verilator_root

    verilator_bin = shutil.which("verilator")
    if verilator_bin is None:
        for candidate in (
            "/opt/homebrew/bin/verilator",
            "/usr/local/bin/verilator",
            "/usr/bin/verilator",
        ):
            if Path(candidate).is_file():
                verilator_bin = candidate
                break
    if verilator_bin is None:
        _logger.error("verilator not found in PATH or common locations")
        sys.exit(1)
    return verilator_bin, None


def _find_verilator_root(verilator_bin: str) -> str | None:
    bin_path = Path(verilator_bin)

    for env_var in ("TEST_SRCDIR", "RUNFILES_DIR"):
        runfiles_dir = os.environ.get(env_var)
        if not runfiles_dir:
            continue
        for repo_name in ("verilator+", "verilator"):
            candidate = Path(runfiles_dir) / repo_name
            if (candidate / "include" / "verilated.h").is_file():
                return str(candidate.resolve())

    runfiles_dir = bin_path.parent / (bin_path.name + ".runfiles")
    if runfiles_dir.is_dir():
        for entry in runfiles_dir.iterdir():
            if (
                entry.name.startswith("verilator")
                and (entry / "include" / "verilated.h").is_file()
            ):
                return str(entry.resolve())

    root_candidate = bin_path.parent.parent
    if (root_candidate / "include" / "verilated.h").is_file():
        return str(root_candidate)

    _logger.warning("Could not determine VERILATOR_ROOT for %s", verilator_bin)
    return None
