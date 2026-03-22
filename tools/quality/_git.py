"""Shared git helpers for quality scripts."""

from __future__ import annotations

import subprocess
from pathlib import Path


def _repo_root() -> Path:
    result = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        check=True,
        capture_output=True,
        text=True,
    )
    return Path(result.stdout.strip())


def _staged_files(repo_root: Path) -> list[Path]:
    result = subprocess.run(
        ["git", "diff", "--cached", "--name-only", "--diff-filter=ACMR"],
        check=True,
        capture_output=True,
        text=True,
        cwd=repo_root,
    )
    return [repo_root / line for line in result.stdout.splitlines() if line.strip()]
