"""Unit tests for tools.quality.run_refactor_matrix."""

from __future__ import annotations

import argparse
from types import SimpleNamespace
from typing import TYPE_CHECKING

from tools.quality import run_refactor_matrix

if TYPE_CHECKING:
    import pytest


def test_visualizer_suite_prefers_pnpm_and_falls_back_to_npm(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    executed: list[tuple[str, ...]] = []

    def fake_which(tool: str) -> str | None:
        if tool == "pnpm":
            return None
        if tool == "npm":
            return "/usr/bin/npm"
        return f"/usr/bin/{tool}"

    def fake_run(argv: tuple[str, ...], **_kwargs: object) -> SimpleNamespace:
        executed.append(tuple(argv))
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(run_refactor_matrix.shutil, "which", fake_which)
    monkeypatch.setattr(run_refactor_matrix.subprocess, "run", fake_run)
    monkeypatch.setattr(
        run_refactor_matrix,
        "_parse_args",
        lambda: argparse.Namespace(
            suite=["visualizer"],
            allow_missing_tools=False,
        ),
    )

    assert run_refactor_matrix.main() == 0
    assert executed == [
        (
            "npm",
            "--prefix",
            "tapa-visualizer",
            "run",
            "lint",
            "--",
            "src",
        ),
        ("npm", "--prefix", "tapa-visualizer", "run", "build"),
    ]
    assert "pnpm" not in capsys.readouterr().err


def test_missing_tools_fail_without_allow_missing(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.setattr(run_refactor_matrix.shutil, "which", lambda _: None)
    monkeypatch.setattr(
        run_refactor_matrix,
        "_parse_args",
        lambda: argparse.Namespace(
            suite=["visualizer"],
            allow_missing_tools=False,
        ),
    )

    assert run_refactor_matrix.main() == 1
    assert "ERROR: Required tool(s)" in capsys.readouterr().err
