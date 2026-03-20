"""Enforce file-size budgets for key source trees."""

from __future__ import annotations

import argparse
import subprocess
from dataclasses import dataclass
from pathlib import Path

MIN_JS_PATH_PARTS = 3
PYTHON_LOC_LIMIT = 450
JS_LOC_LIMIT = 300


@dataclass(frozen=True)
class BudgetViolation:
    """A single file-size budget violation."""

    path: Path
    lines: int
    limit: int
    rule: str


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


def _is_test_python(path: Path) -> bool:
    name = path.name
    if name.startswith("test_") or name.endswith("_test.py"):
        return True
    parts = set(path.parts)
    return "tests" in parts or "test" in parts


def _python_budget_target(path: Path) -> bool:
    if path.suffix != ".py":
        return False
    if "third_party" in path.parts:
        return False
    if "tapa" not in path.parts:
        return False
    return not _is_test_python(path)


def _js_budget_target(path: Path) -> bool:
    if path.suffix not in {".js", ".mjs", ".cjs"}:
        return False
    return (
        len(path.parts) >= MIN_JS_PATH_PARTS
        and path.parts[0] == "tapa-visualizer"
        and path.parts[1] == "src"
    )


def _line_count(path: Path) -> int:
    with path.open(encoding="utf-8") as file:
        return sum(1 for _ in file)


def _violations(repo_root: Path, candidates: list[Path]) -> list[BudgetViolation]:
    violations: list[BudgetViolation] = []
    for path in candidates:
        if not path.exists() or not path.is_file():
            continue
        rel = path.relative_to(repo_root)
        lines = _line_count(path)
        if _python_budget_target(rel) and lines > PYTHON_LOC_LIMIT:
            violations.append(
                BudgetViolation(
                    path=rel,
                    lines=lines,
                    limit=PYTHON_LOC_LIMIT,
                    rule="tapa Python file",
                ),
            )
        if _js_budget_target(rel) and lines > JS_LOC_LIMIT:
            violations.append(
                BudgetViolation(
                    path=rel,
                    lines=lines,
                    limit=JS_LOC_LIMIT,
                    rule="visualizer JS file",
                ),
            )
    return violations


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


def main() -> int:
    args = _parse_args()
    repo_root = _repo_root()
    if args.mode == "staged":
        candidates = _staged_files(repo_root)
    else:
        candidates = [(repo_root / path).resolve() for path in args.paths]
    violations = _violations(repo_root, candidates)
    if not violations:
        return 0
    for violation in violations:
        pass
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
