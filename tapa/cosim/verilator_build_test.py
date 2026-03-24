"""Tests for Verilator build and launch helpers."""

import re
from pathlib import Path
from tempfile import TemporaryDirectory
from textwrap import dedent
from typing import TYPE_CHECKING
from unittest.mock import patch

from tapa.cosim.verilator_build import generate_build_script, launch_verilator

if TYPE_CHECKING:
    import pytest


_FIXTURES = Path(__file__).with_name("testdata").joinpath("verilator_empty")


def _canonicalize(text: str) -> str:
    return re.sub(r"\s+", "", text)


def test_generate_build_script_matches_golden_output() -> None:
    with patch(
        "tapa.cosim.verilator_build._find_verilator",
        return_value=("/verilator/bin/verilator", None),
    ):
        rendered = _canonicalize(generate_build_script("top"))

    expected = _canonicalize(_FIXTURES.joinpath("build.sh").read_text(encoding="utf-8"))
    assert rendered == expected


def test_launch_verilator_runs_build_script_and_binary(
    capsys: "pytest.CaptureFixture[str]",
) -> None:
    with TemporaryDirectory() as tb_dir:
        tb_path = Path(tb_dir)
        build_script = tb_path / "build.sh"
        build_script.write_text(
            dedent(
                """\
                #!/bin/sh
                set -eu
                mkdir -p obj_dir
                cat > obj_dir/Vtop <<'EOF'
                #!/bin/sh
                printf 'sim ok\\n'
                EOF
                chmod +x obj_dir/Vtop
                """
            ),
            encoding="utf-8",
        )
        build_script.chmod(0o755)

        launch_verilator("top", tb_dir)

    captured = capsys.readouterr()
    assert "sim ok" in captured.out
