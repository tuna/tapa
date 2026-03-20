"""Prevent Ruff finding count regression on modified Python files."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class LintDelta:
    """Lint findings delta for one file."""

    path: Path
    before: int
    after: int


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


def _head_has_file(repo_root: Path, rel_path: Path) -> bool:
    result = subprocess.run(
        ["git", "cat-file", "-e", f"HEAD:{rel_path.as_posix()}"],
        check=False,
        cwd=repo_root,
        capture_output=True,
        text=True,
    )
    return result.returncode == 0


def _load_head_file(repo_root: Path, rel_path: Path) -> str:
    result = subprocess.run(
        ["git", "show", f"HEAD:{rel_path.as_posix()}"],
        check=True,
        capture_output=True,
        text=True,
        cwd=repo_root,
    )
    return result.stdout


def _ruff_count(target: Path, config: Path) -> int:
    result = subprocess.run(
        [
            "ruff",
            "check",
            "--output-format",
            "json",
            "--config",
            str(config),
            str(target),
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    # ruff returns non-zero when findings exist; treat both 0 and 1 as valid.
    if result.returncode not in {0, 1}:
        sys.stderr.write((result.stderr.strip() or result.stdout.strip()) + "\n")
        msg = f"ruff failed for {target}"
        raise RuntimeError(msg)
    payload = result.stdout.strip() or "[]"
    findings = json.loads(payload)
    return len(findings)


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--mode",
        choices=("staged", "paths"),
        default="paths",
        help="Source of file list. 'paths' uses positional filenames.",
    )
    parser.add_argument("paths", nargs="*", help="Candidate files to check")
    return parser.parse_args()


def _python_paths(repo_root: Path, args: argparse.Namespace) -> list[Path]:
    if args.mode == "staged":
        candidates = _staged_files(repo_root)
    else:
        candidates = [(repo_root / path).resolve() for path in args.paths]
    python_paths = [
        path
        for path in candidates
        if path.suffix == ".py" and path.exists() and path.is_file()
    ]
    return sorted(python_paths)


def main() -> int:
    args = _parse_args()
    repo_root = _repo_root()
    config = repo_root / "pyproject.toml"
    deltas: list[LintDelta] = []
    for path in _python_paths(repo_root, args):
        rel = path.relative_to(repo_root)
        after = _ruff_count(path, config)
        before = 0
        if _head_has_file(repo_root, rel):
            with tempfile.TemporaryDirectory(prefix="lint-budget-") as tmp_dir:
                tmp_path = Path(tmp_dir) / rel
                tmp_path.parent.mkdir(parents=True, exist_ok=True)
                tmp_path.write_text(_load_head_file(repo_root, rel), encoding="utf-8")
                before = _ruff_count(tmp_path, config)
        if after > before:
            deltas.append(LintDelta(path=rel, before=before, after=after))
    if not deltas:
        return 0
    sys.stderr.write("Lint budget regression detected (ruff findings increased):\n")
    for delta in deltas:
        sys.stderr.write(
            f"  - {delta.path}: before={delta.before} -> after={delta.after}\n",
        )
    sys.stderr.write(f"Total regressions: {len(deltas)}\n")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
