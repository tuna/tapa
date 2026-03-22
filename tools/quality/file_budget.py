"""Enforce file-size budgets for key source trees."""

from __future__ import annotations

import argparse
import ast
import json
import sys
from dataclasses import dataclass
from pathlib import Path

from _git import _repo_root, _staged_files  # noqa: PLC2701

MIN_JS_PATH_PARTS = 3
PYTHON_LOC_LIMIT = 450
JS_LOC_LIMIT = 300
PYTHON_FUNCTION_LOC_LIMIT = 90
FILE_ALLOWLIST_PATH = Path("tools/quality/file_budget_allowlist.txt")
FUNCTION_ALLOWLIST_PATH = Path("tools/quality/function_length_allowlist.txt")


@dataclass(frozen=True)
class BudgetViolation:
    path: Path
    lines: int
    limit: int
    rule: str


@dataclass(frozen=True)
class FunctionViolation:
    path: Path
    symbol: str
    lines: int
    limit: int


@dataclass(frozen=True)
class BudgetBaseline:
    file_violations: dict[str, BudgetViolation]
    function_violations: dict[str, FunctionViolation]


def _all_target_files(repo_root: Path) -> list[Path]:
    candidates: list[Path] = []
    candidates.extend((repo_root / "tapa").rglob("*.py"))
    candidates.extend((repo_root / "tapa-visualizer" / "src").rglob("*.js"))
    candidates.extend((repo_root / "tapa-visualizer" / "src").rglob("*.mjs"))
    candidates.extend((repo_root / "tapa-visualizer" / "src").rglob("*.cjs"))
    return sorted(candidates)


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


def _load_allowlist(path: Path) -> set[str]:
    if not path.exists():
        return set()
    entries: set[str] = set()
    with path.open(encoding="utf-8") as file:
        for raw_line in file:
            line = raw_line.strip()
            if not line or line.startswith("#"):
                continue
            entries.add(line)
    return entries


def _load_baseline(path: Path) -> BudgetBaseline:
    if not path.exists():
        raise FileNotFoundError(path)

    payload = json.loads(path.read_text(encoding="utf-8"))

    file_violations: dict[str, BudgetViolation] = {}
    for entry in payload.get("file_violations", []):
        violation = BudgetViolation(
            path=Path(entry["path"]),
            lines=int(entry["lines"]),
            limit=int(entry["limit"]),
            rule=str(entry.get("rule", "tapa Python file")),
        )
        file_violations[violation.path.as_posix()] = violation

    function_violations: dict[str, FunctionViolation] = {}
    for entry in payload.get("function_violations", []):
        violation = FunctionViolation(
            path=Path(entry["path"]),
            symbol=str(entry["symbol"]),
            lines=int(entry["lines"]),
            limit=int(entry["limit"]),
        )
        key = f"{violation.path.as_posix()}:{violation.symbol}"
        function_violations[key] = violation

    return BudgetBaseline(
        file_violations=file_violations,
        function_violations=function_violations,
    )


def _baseline_regressions(
    file_violations: list[BudgetViolation],
    function_violations: list[FunctionViolation],
    baseline: BudgetBaseline,
) -> list[str]:
    regressions: list[str] = []

    for violation in file_violations:
        key = violation.path.as_posix()
        accepted = baseline.file_violations.get(key)
        if accepted is None:
            regressions.append(
                f"  - [baseline file] {violation.path}: {violation.lines} LOC "
                f"> limit {violation.limit} and is not present in the baseline",
            )
            continue
        if violation.lines > accepted.lines:
            regressions.append(
                f"  - [baseline file] {violation.path}: {violation.lines} LOC "
                f"> baseline {accepted.lines} LOC",
            )

    for violation in function_violations:
        key = f"{violation.path.as_posix()}:{violation.symbol}"
        accepted = baseline.function_violations.get(key)
        if accepted is None:
            regressions.append(
                f"  - [baseline function] {violation.path}:{violation.symbol} "
                f"{violation.lines} LOC > limit {violation.limit} and is not "
                "present in the baseline",
            )
            continue
        if violation.lines > accepted.lines:
            regressions.append(
                f"  - [baseline function] {violation.path}:{violation.symbol} "
                f"{violation.lines} LOC > baseline {accepted.lines} LOC",
            )

    return regressions


def _function_violations(
    path: Path,
    rel: Path,
    function_limit: int,
    function_allowlist: set[str],
) -> list[FunctionViolation]:
    source = path.read_text(encoding="utf-8")
    tree = ast.parse(source, filename=str(path))
    violations: list[FunctionViolation] = []

    class _Visitor(ast.NodeVisitor):
        def __init__(self) -> None:
            self.stack: list[str] = []

        def _visit_function(
            self,
            node: ast.FunctionDef | ast.AsyncFunctionDef,
        ) -> None:
            self.stack.append(node.name)
            symbol = ".".join(self.stack)
            allowlist_key = f"{rel.as_posix()}:{symbol}"
            line_count = (node.end_lineno or node.lineno) - node.lineno + 1
            if line_count > function_limit and allowlist_key not in function_allowlist:
                violations.append(
                    FunctionViolation(
                        path=rel,
                        symbol=symbol,
                        lines=line_count,
                        limit=function_limit,
                    ),
                )
            self.generic_visit(node)
            self.stack.pop()

        def visit_ClassDef(self, node: ast.ClassDef) -> None:
            self.stack.append(node.name)
            self.generic_visit(node)
            self.stack.pop()

        def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
            self._visit_function(node)

        def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
            self._visit_function(node)

    _Visitor().visit(tree)
    return violations


def _file_violations(
    repo_root: Path,
    candidates: list[Path],
    file_allowlist: set[str],
) -> list[BudgetViolation]:
    violations: list[BudgetViolation] = []
    for path in candidates:
        if not path.exists() or not path.is_file():
            continue
        rel = path.relative_to(repo_root)
        lines = _line_count(path)
        if (
            _python_budget_target(rel)
            and lines > PYTHON_LOC_LIMIT
            and rel.as_posix() not in file_allowlist
        ):
            violations.append(
                BudgetViolation(
                    path=rel,
                    lines=lines,
                    limit=PYTHON_LOC_LIMIT,
                    rule="tapa Python file",
                ),
            )
        if (
            _js_budget_target(rel)
            and lines > JS_LOC_LIMIT
            and rel.as_posix() not in file_allowlist
        ):
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
        choices=("staged", "paths", "all"),
        default="paths",
        help="Source of file list. 'paths' uses positional filenames.",
    )
    parser.add_argument(
        "--python-function-loc-limit",
        type=int,
        default=PYTHON_FUNCTION_LOC_LIMIT,
        help=(
            "Max LOC per Python function/method "
            f"(default: {PYTHON_FUNCTION_LOC_LIMIT})."
        ),
    )
    parser.add_argument(
        "--disable-function-budget",
        action="store_true",
        help="Disable Python function-length checks.",
    )
    parser.add_argument(
        "--file-allowlist",
        default=str(FILE_ALLOWLIST_PATH),
        help="Path to file LOC exception allowlist (one relative path per line).",
    )
    parser.add_argument(
        "--function-allowlist",
        default=str(FUNCTION_ALLOWLIST_PATH),
        help=(
            "Path to function LOC exception allowlist "
            "(format: relative/path.py:qualified.symbol)."
        ),
    )
    parser.add_argument(
        "--baseline",
        help=(
            "Path to a repo-wide baseline snapshot of accepted budget debt. "
            "When set, only regressions beyond the snapshot fail."
        ),
    )
    parser.add_argument("paths", nargs="*", help="Candidate files to check")
    return parser.parse_args()


def _candidate_paths(repo_root: Path, args: argparse.Namespace) -> list[Path]:
    if args.mode == "staged":
        return _staged_files(repo_root)
    if args.mode == "all":
        return _all_target_files(repo_root)
    return [(repo_root / path).resolve() for path in args.paths]


def _function_budget_violations_for_candidates(
    repo_root: Path,
    candidates: list[Path],
    *,
    disabled: bool,
    function_limit: int,
    function_allowlist: set[str],
) -> list[FunctionViolation]:
    if disabled:
        return []

    violations: list[FunctionViolation] = []
    for path in candidates:
        if not path.exists() or not path.is_file():
            continue
        rel = path.relative_to(repo_root)
        if _python_budget_target(rel):
            violations.extend(
                _function_violations(
                    path=path,
                    rel=rel,
                    function_limit=function_limit,
                    function_allowlist=function_allowlist,
                ),
            )
    return violations


def _baseline_exit_code(
    *,
    file_violations: list[BudgetViolation],
    function_violations: list[FunctionViolation],
    baseline: BudgetBaseline | None,
    disable_function_budget: bool,
) -> int | None:
    if baseline is None:
        return None
    if disable_function_budget:
        sys.stderr.write(
            "ERROR: --baseline cannot be combined with --disable-function-budget\n",
        )
        return 1
    regressions = _baseline_regressions(
        file_violations=file_violations,
        function_violations=function_violations,
        baseline=baseline,
    )
    if not regressions:
        return 0
    sys.stderr.write("File budget baseline regressions detected:\n")
    for regression in regressions:
        sys.stderr.write(f"{regression}\n")
    sys.stderr.write(f"Total regressions: {len(regressions)}\n")
    return 1


def _report_violations(
    file_violations: list[BudgetViolation],
    function_violations: list[FunctionViolation],
) -> int:
    total_violations = len(file_violations) + len(function_violations)
    if total_violations == 0:
        return 0

    sys.stderr.write("File budget violations detected:\n")
    for violation in file_violations:
        sys.stderr.write(
            f"  - [{violation.rule}] {violation.path}: "
            f"{violation.lines} LOC > limit {violation.limit}. "
            "Hint: split the file or add a temporary allowlist entry.\n",
        )
    for violation in function_violations:
        sys.stderr.write(
            f"  - [python function] {violation.path}:{violation.symbol} "
            f"{violation.lines} LOC > limit {violation.limit}. "
            "Hint: extract helpers or add a temporary allowlist entry.\n",
        )
    sys.stderr.write(
        "Total violations: "
        f"{total_violations} "
        f"(file={len(file_violations)}, function={len(function_violations)})\n",
    )
    return 1


def main() -> int:
    args = _parse_args()
    repo_root = _repo_root()
    baseline = _load_baseline(repo_root / args.baseline) if args.baseline else None

    candidates = _candidate_paths(repo_root, args)
    file_allowlist = _load_allowlist(repo_root / args.file_allowlist)
    function_allowlist = _load_allowlist(repo_root / args.function_allowlist)
    file_violations = _file_violations(repo_root, candidates, file_allowlist)
    function_violations = _function_budget_violations_for_candidates(
        repo_root,
        candidates,
        disabled=args.disable_function_budget,
        function_limit=args.python_function_loc_limit,
        function_allowlist=function_allowlist,
    )

    baseline_exit_code = _baseline_exit_code(
        file_violations=file_violations,
        function_violations=function_violations,
        baseline=baseline,
        disable_function_budget=args.disable_function_budget,
    )
    if baseline_exit_code is not None:
        return baseline_exit_code
    return _report_violations(file_violations, function_violations)


if __name__ == "__main__":
    raise SystemExit(main())
