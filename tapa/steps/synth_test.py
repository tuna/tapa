"""Characterization tests for tapa/steps/synth.py."""

from unittest.mock import MagicMock, patch

import click
import pytest
from click.testing import CliRunner

from tapa.backend.xilinx import parse_device_info
from tapa.steps.synth import synth

# ---------------------------------------------------------------------------
# parse_device_info unit tests
# ---------------------------------------------------------------------------


def test_parse_device_info_with_part_num_and_clock_period() -> None:
    """With part_num + clock_period (no platform), returns both keys."""
    result = parse_device_info(
        (None, "--platform"),
        ("xcu250-figd2104-2L-e", "--part-num"),
        (3.33, "--clock-period"),
        lambda msg: (_ for _ in ()).throw(ValueError(msg)),
    )
    assert "part_num" in result
    assert "clock_period" in result
    assert result["part_num"] == "xcu250-figd2104-2L-e"
    assert result["clock_period"] is not None
    assert result["clock_period"]  # non-empty string


def test_parse_device_info_no_device_raises() -> None:
    """With no platform and no part_num/clock_period, on_error is invoked."""
    errors: list[str] = []

    def capture_error(msg: str):  # noqa: ANN202
        errors.append(msg)
        raise click.BadArgumentUsage(msg)

    with pytest.raises(click.BadArgumentUsage):
        parse_device_info(
            (None, "--platform"),
            (None, "--part-num"),
            (None, "--clock-period"),
            capture_error,
        )

    assert len(errors) >= 1


# ---------------------------------------------------------------------------
# synth routing tests via CliRunner
# ---------------------------------------------------------------------------

_MINIMAL_SYNTH_ARGS = [
    "--part-num",
    "xcu250-figd2104-2L-e",
    "--clock-period",
    "3.33",
]


def _make_program_mock() -> MagicMock:
    """Return a MagicMock that quacks like a tapa.core.Program."""
    prog = MagicMock()
    prog.work_dir = "/tmp/tapa_test_work"
    prog.get_rtl_templates_info.return_value = {}
    return prog


def test_synth_routes_to_run_aie_for_aie_target() -> None:
    """synth() calls program.run_aie (not run_hls) when target is xilinx-aie."""
    program = _make_program_mock()

    with (
        patch("tapa.steps.synth.load_tapa_program", return_value=program),
        patch(
            "tapa.steps.synth.load_persistent_context",
            return_value={"target": "xilinx-aie"},
        ),
        patch(
            "tapa.steps.synth.parse_device_info",
            return_value={
                "part_num": "xcu250-figd2104-2L-e",
                "clock_period": "3.33",
            },
        ),
        patch("tapa.steps.synth.store_persistent_context"),
        patch("tapa.steps.synth.is_pipelined"),
    ):
        runner = CliRunner()
        result = runner.invoke(
            synth,
            [
                "--part-num",
                "xcu250-figd2104-2L-e",
                "--clock-period",
                "3.33",
                "--platform",
                "/fake/platform",
            ],
            obj={"work-dir": "/tmp/tapa_test_work"},
            catch_exceptions=False,
        )

    assert result.exit_code == 0, result.output
    program.run_aie.assert_called_once()
    program.run_hls.assert_not_called()


def test_synth_routes_to_run_hls_for_hls_target() -> None:
    """synth() calls program.run_hls + generate_task_rtl when target is xilinx-hls."""
    program = _make_program_mock()

    with (
        patch("tapa.steps.synth.load_tapa_program", return_value=program),
        patch(
            "tapa.steps.synth.load_persistent_context",
            return_value={"target": "xilinx-hls"},
        ),
        patch(
            "tapa.steps.synth.parse_device_info",
            return_value={
                "part_num": "xcu250-figd2104-2L-e",
                "clock_period": "3.33",
            },
        ),
        patch("tapa.steps.synth.store_persistent_context"),
        patch("tapa.steps.synth.is_pipelined"),
    ):
        runner = CliRunner()
        result = runner.invoke(
            synth,
            _MINIMAL_SYNTH_ARGS,
            obj={"work-dir": "/tmp/tapa_test_work"},
            catch_exceptions=False,
        )

    assert result.exit_code == 0, result.output
    program.run_hls.assert_called_once()
    program.generate_task_rtl.assert_called_once()
    program.run_aie.assert_not_called()
