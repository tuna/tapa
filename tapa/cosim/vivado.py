"""Generate a Vivado TCL script for cosimulation."""

from __future__ import annotations

import logging
import re
import shlex
import subprocess
from typing import TYPE_CHECKING

from packaging.version import Version

from tapa.common import paths
from tapa.remote.config import RemoteConfig, get_remote_config
from tapa.remote.ssh import run_ssh_with_stdout

if TYPE_CHECKING:
    from tapa.cosim.config_preprocess import CosimConfig

_logger = logging.getLogger().getChild(__name__)


def get_vivado_version() -> str:
    """Return the Vivado version."""
    config = get_remote_config()
    if config is not None:
        return _get_vivado_version_remote(config)
    return _get_vivado_version_local()


def _get_vivado_version_local() -> str:
    """Return the Vivado version from local installation."""
    command = ["vivado", "-version"]
    try:
        output = subprocess.check_output(command, stderr=subprocess.STDOUT)
        return _parse_vivado_version(output.decode("utf-8"))

    except FileNotFoundError:
        error = "Vivado not found. Please add Vivado to PATH."
        raise FileNotFoundError(error)

    except subprocess.CalledProcessError as e:
        error = f"Failed to get Vivado version: {e.output.decode('utf-8')}"
        raise ValueError(error) from e


def _get_vivado_version_remote(config: RemoteConfig) -> str:
    """Return the Vivado version from remote host."""
    cmd_parts = []
    if config.xilinx_settings:
        cmd_parts.append(f"source {shlex.quote(config.xilinx_settings)}")
    cmd_parts.append("vivado -version")
    full_cmd = " ; ".join(cmd_parts)

    exit_status, stdout, stderr = run_ssh_with_stdout(
        config, f"bash -c {shlex.quote(full_cmd)}"
    )
    output = stdout.decode("utf-8")

    if exit_status != 0:
        err = stderr.decode("utf-8", errors="replace")
        error = f"Failed to get Vivado version from remote: {err}"
        raise ValueError(error)

    return _parse_vivado_version(output)


def _parse_vivado_version(version_lines: str) -> str:
    """Parse vivado version string from version output."""
    match = re.search(r"vivado v(\d+\.\d+)", version_lines, re.IGNORECASE)
    if match is None:
        error = f"Failed to parse Vivado version from:\n{version_lines}"
        raise ValueError(error)
    version = match.group(1)
    _logger.info("Vivado version: %s", version)
    return version


def get_vivado_tcl(
    config: CosimConfig,
    tb_rtl_path: str,
    save_waveform: bool,
    start_gui: bool,
) -> list[str]:
    """Generate a Vivado TCL script for cosimulation."""
    dpi_version = (
        "tapa_fast_cosim_dpi_xv"
        if Version(get_vivado_version()) >= Version("2024.2")
        else "tapa_fast_cosim_dpi_legacy_rdi"
    )

    tapa_hdl_path = config.verilog_path

    script = []

    part_num = config.part_num

    if not part_num:
        msg = (
            "part_num is not set. Either provide an xo that contains HLS reports or "
            "use the --xosim-part-num option to specify the part number."
        )
        raise ValueError(msg)

    script.append(f"create_project -force tapa-fast-cosim ./vivado -part {part_num}")
    script.append(f'set ORIG_RTL_PATH "{tapa_hdl_path}"')

    for suffix in (".v", ".sv", ".dat"):
        for loc in (f"${{ORIG_RTL_PATH}}/*{suffix}", f"${{ORIG_RTL_PATH}}/*/*{suffix}"):
            script.append(f"set rtl_files [glob -nocomplain {loc}]")
            script.append(
                'if {$rtl_files ne ""} '
                "{add_files -norecurse -scan_for_includes ${rtl_files} }"
            )

    for loc in (r"${ORIG_RTL_PATH}/*.tcl", r"${ORIG_RTL_PATH}/*/*.tcl"):
        script.append(f"set tcl_files [glob -nocomplain {loc}]")
        script.append(r"foreach ip_tcl ${tcl_files} { source ${ip_tcl} }")

    for loc in (r"${ORIG_RTL_PATH}/*/*.xci", r"${ORIG_RTL_PATH}/*.xci"):
        script.append(f"set xci_ip_files [glob -nocomplain {loc}]")
        script.append(
            'if {$xci_ip_files ne ""} '
            "{add_files -norecurse -scan_for_includes ${xci_ip_files} }"
        )

    script.append("upgrade_ip -quiet [get_ips *]")

    script.append(f"set tb_files [glob {tb_rtl_path}/*.v {tb_rtl_path}/*.sv]")
    script.append(r"set_property SOURCE_SET sources_1 [get_filesets sim_1]")
    script.append(r"add_files -fileset sim_1 -norecurse -scan_for_includes ${tb_files}")

    script.append("set_property top test [get_filesets sim_1]")
    script.append("set_property top_lib xil_defaultlib [get_filesets sim_1]")

    dpi_library_dir = paths.find_resource("tapa-fast-cosim-dpi-lib")
    if dpi_library_dir is None:
        _logger.fatal("DPI directory not found")
    else:
        _logger.debug("DPI directory: %s", dpi_library_dir)
        script.append(
            "set_property -name {xelab.more_options} "
            f"-value {{-sv_root {dpi_library_dir} -sv_lib {dpi_version}}} "
            "-objects [get_filesets sim_1]"
        )

    if save_waveform or start_gui:
        script.append(
            r"set_property -name {xsim.simulate.log_all_signals} "
            r"-value {true} -objects [get_filesets sim_1]"
        )
    if save_waveform:
        script.append(
            r"set_property -name {xsim.simulate.wdb} "
            r"-value {wave.wdb} -objects [get_filesets sim_1]"
        )

    script.append(r"launch_simulation")
    script.append(r"run all")

    return script
