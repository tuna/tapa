"""Unit tests for tools.quality.lint_budget."""

from __future__ import annotations

import argparse
from typing import TYPE_CHECKING

from tools.quality import lint_budget

if TYPE_CHECKING:
    from pathlib import Path

    import pytest


def test_no_regression_returns_success(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    target = tmp_path / "tapa" / "module.py"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("x = 1\n", encoding="utf-8")

    monkeypatch.setattr(
        lint_budget,
        "_parse_args",
        lambda: argparse.Namespace(mode="paths", paths=[]),
    )
    monkeypatch.setattr(lint_budget, "_repo_root", lambda: tmp_path)
    monkeypatch.setattr(lint_budget, "_python_paths", lambda *_: [target])
    monkeypatch.setattr(lint_budget, "_head_has_file", lambda *_: True)
    monkeypatch.setattr(lint_budget, "_load_head_file", lambda *_: "x = 1\n")

    counts = iter([1, 1])
    monkeypatch.setattr(lint_budget, "_ruff_count", lambda *_: next(counts))

    assert lint_budget.main() == 0
    assert "Lint budget regression detected" not in capsys.readouterr().err


def test_regression_prints_delta_and_fails(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    target = tmp_path / "tapa" / "module.py"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("x = 1\n", encoding="utf-8")

    monkeypatch.setattr(
        lint_budget,
        "_parse_args",
        lambda: argparse.Namespace(mode="paths", paths=[]),
    )
    monkeypatch.setattr(lint_budget, "_repo_root", lambda: tmp_path)
    monkeypatch.setattr(lint_budget, "_python_paths", lambda *_: [target])
    monkeypatch.setattr(lint_budget, "_head_has_file", lambda *_: True)
    monkeypatch.setattr(lint_budget, "_load_head_file", lambda *_: "x = 1\n")

    counts = iter([3, 1])
    monkeypatch.setattr(lint_budget, "_ruff_count", lambda *_: next(counts))

    assert lint_budget.main() == 1
    stderr = capsys.readouterr().err
    assert "Lint budget regression detected" in stderr
    assert "before=1 -> after=3" in stderr
