"""Characterization tests for vivado TCL script generation."""

from collections.abc import Generator
from unittest.mock import patch

import pytest

from tapa.cosim.config_preprocess import CosimConfig
from tapa.cosim.vivado import get_vivado_tcl


def _make_config(part_num: str = "xcu250-figd2104-2L-e") -> CosimConfig:
    return CosimConfig(
        xo_path="/fake/kernel.xo",
        verilog_path="/fake/rtl",
        top_name="mykernel",
        part_num=part_num,
    )


@pytest.fixture(autouse=True)
def _mock_vivado_version() -> Generator[None]:
    """Prevent any attempt to run vivado or SSH during tests."""
    with patch("tapa.cosim.vivado.get_vivado_version", return_value="2024.2"):
        yield


@pytest.fixture(autouse=True)
def _mock_find_resource() -> Generator[None]:
    """Prevent filesystem lookup for DPI library during tests."""
    with patch("tapa.cosim.vivado.paths.find_resource", return_value="/fake/dpi/lib"):
        yield


def test_get_vivado_tcl_returns_nonempty_list() -> None:
    """get_vivado_tcl must return a non-empty list of TCL commands."""
    script = get_vivado_tcl(
        config=_make_config(),
        tb_rtl_path="/fake/tb",
        save_waveform=False,
        start_gui=False,
    )
    assert isinstance(script, list)
    assert len(script) > 0, "Expected at least one TCL command"


def test_get_vivado_tcl_contains_create_project() -> None:
    """Script must contain a create_project command with the part number."""
    config = _make_config(part_num="xcu250-figd2104-2L-e")
    script = get_vivado_tcl(
        config=config,
        tb_rtl_path="/fake/tb",
        save_waveform=False,
        start_gui=False,
    )
    combined = "\n".join(script)
    assert "create_project" in combined
    assert "xcu250-figd2104-2L-e" in combined


def test_get_vivado_tcl_contains_launch_simulation() -> None:
    """Script must contain launch_simulation to actually run the simulation."""
    script = get_vivado_tcl(
        config=_make_config(),
        tb_rtl_path="/fake/tb",
        save_waveform=False,
        start_gui=False,
    )
    assert any("launch_simulation" in line for line in script)


def test_get_vivado_tcl_contains_run_all() -> None:
    """Script must contain 'run all' to execute the full simulation."""
    script = get_vivado_tcl(
        config=_make_config(),
        tb_rtl_path="/fake/tb",
        save_waveform=False,
        start_gui=False,
    )
    assert any("run all" in line for line in script)


def test_get_vivado_tcl_no_waveform_by_default() -> None:
    """Without save_waveform, no xsim.simulate.wdb property should be set."""
    script = get_vivado_tcl(
        config=_make_config(),
        tb_rtl_path="/fake/tb",
        save_waveform=False,
        start_gui=False,
    )
    combined = "\n".join(script)
    assert "xsim.simulate.wdb" not in combined


def test_get_vivado_tcl_save_waveform_sets_wdb() -> None:
    """When save_waveform=True, the script must set the wdb property."""
    script = get_vivado_tcl(
        config=_make_config(),
        tb_rtl_path="/fake/tb",
        save_waveform=True,
        start_gui=False,
    )
    combined = "\n".join(script)
    assert "xsim.simulate.wdb" in combined


def test_get_vivado_tcl_missing_part_num_raises() -> None:
    """If config.part_num is empty, get_vivado_tcl must raise ValueError."""
    config = _make_config()
    config.part_num = None

    with pytest.raises(ValueError, match="part_num is not set"):
        get_vivado_tcl(
            config=config,
            tb_rtl_path="/fake/tb",
            save_waveform=False,
            start_gui=False,
        )


def test_get_vivado_tcl_tb_rtl_path_appears_in_script() -> None:
    """The tb_rtl_path must be referenced in the generated TCL script."""
    tb_path = "/my/custom/tb/path"
    script = get_vivado_tcl(
        config=_make_config(),
        tb_rtl_path=tb_path,
        save_waveform=False,
        start_gui=False,
    )
    combined = "\n".join(script)
    assert tb_path in combined


def test_get_vivado_tcl_uses_rust_xsim_dpi_library_name() -> None:
    """The fallback xsim TCL should reference the Rust DPI library basename."""
    script = get_vivado_tcl(
        config=_make_config(),
        tb_rtl_path="/fake/tb",
        save_waveform=False,
        start_gui=False,
    )
    combined = "\n".join(script)
    assert "-sv_lib frt_dpi_xsim" in combined


def test_get_vivado_tcl_uses_modern_xsim_elab_property_for_new_vivado() -> None:
    """Vivado 2024.2+ uses the scoped xsim elaborate property name."""
    script = get_vivado_tcl(
        config=_make_config(),
        tb_rtl_path="/fake/tb",
        save_waveform=False,
        start_gui=False,
    )
    combined = "\n".join(script)
    assert "xsim.elaborate.xelab.more_options" in combined
