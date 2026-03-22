"""Unit tests for tools.quality.file_budget."""

from __future__ import annotations

import json
from pathlib import Path

from tools.quality import file_budget


def _write_lines(path: Path, count: int) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(f"line_{i}" for i in range(count)), encoding="utf-8")


def test_file_loc_violation_and_allowlist(tmp_path: Path) -> None:
    repo_root = tmp_path
    target = repo_root / "tapa" / "big.py"
    _write_lines(target, file_budget.PYTHON_LOC_LIMIT + 1)

    violations = file_budget._file_violations(  # noqa: SLF001
        repo_root=repo_root,
        candidates=[target],
        file_allowlist=set(),
    )

    assert len(violations) == 1
    assert violations[0].path == Path("tapa/big.py")

    suppressed = file_budget._file_violations(  # noqa: SLF001
        repo_root=repo_root,
        candidates=[target],
        file_allowlist={"tapa/big.py"},
    )
    assert suppressed == []


def test_function_loc_violation_and_allowlist(tmp_path: Path) -> None:
    repo_root = tmp_path
    target = repo_root / "tapa" / "long_fn.py"
    target.parent.mkdir(parents=True, exist_ok=True)
    long_body = "\n".join("    value += 1" for _ in range(12))
    target.write_text(
        f"def too_long(value):\n{long_body}\n    return value\n",
        encoding="utf-8",
    )

    rel = Path("tapa/long_fn.py")
    violations = file_budget._function_violations(  # noqa: SLF001
        path=target,
        rel=rel,
        function_limit=10,
        function_allowlist=set(),
    )
    assert len(violations) == 1
    assert violations[0].symbol == "too_long"

    suppressed = file_budget._function_violations(  # noqa: SLF001
        path=target,
        rel=rel,
        function_limit=10,
        function_allowlist={"tapa/long_fn.py:too_long"},
    )
    assert suppressed == []


def test_baseline_regressions_allow_existing_debt(tmp_path: Path) -> None:
    baseline_path = tmp_path / "baseline.json"
    baseline_path.write_text(
        json.dumps(
            {
                "file_violations": [],
                "function_violations": [
                    {
                        "path": "tapa/old.py",
                        "symbol": "legacy",
                        "lines": 100,
                        "limit": 90,
                    },
                ],
            },
        ),
        encoding="utf-8",
    )

    baseline = file_budget._load_baseline(baseline_path)  # noqa: SLF001
    current = [
        file_budget.FunctionViolation(
            path=Path("tapa/old.py"),
            symbol="legacy",
            lines=95,
            limit=90,
        ),
    ]
    assert (
        file_budget._baseline_regressions(  # noqa: SLF001
            file_violations=[],
            function_violations=current,
            baseline=baseline,
        )
        == []
    )


def test_baseline_regressions_flag_new_or_growing_debt(
    tmp_path: Path,
) -> None:
    baseline_path = tmp_path / "baseline.json"
    baseline_path.write_text(
        json.dumps(
            {
                "file_violations": [],
                "function_violations": [
                    {
                        "path": "tapa/old.py",
                        "symbol": "legacy",
                        "lines": 100,
                        "limit": 90,
                    },
                ],
            },
        ),
        encoding="utf-8",
    )

    baseline = file_budget._load_baseline(baseline_path)  # noqa: SLF001
    regressions = file_budget._baseline_regressions(  # noqa: SLF001
        file_violations=[],
        function_violations=[
            file_budget.FunctionViolation(
                path=Path("tapa/old.py"),
                symbol="legacy",
                lines=101,
                limit=90,
            ),
            file_budget.FunctionViolation(
                path=Path("tapa/new.py"),
                symbol="fresh",
                lines=95,
                limit=90,
            ),
        ],
        baseline=baseline,
    )

    assert any("baseline 100 LOC" in regression for regression in regressions)
    assert any(
        "not present in the baseline" in regression for regression in regressions
    )
