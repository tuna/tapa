import ast as _ast
import logging
import os
import re
import signal
import subprocess
import sys
from collections.abc import Sequence
from contextlib import suppress
from io import TextIOWrapper
from pathlib import Path

import click
import psutil

from tapa import __version__
from tapa.cosim.common import AXI, Arg, parse_register_addr
from tapa.cosim.config_preprocess import CosimConfig, preprocess_config
from tapa.cosim.templates import (
    get_axi_ram_inst,
    get_axi_ram_module,
    get_axis,
    get_begin,
    get_end,
    get_fifo,
    get_hls_dut,
    get_hls_test_signals,
    get_s_axi_control,
    get_srl_fifo_template,
    get_vitis_dut,
    get_vitis_test_signals,
)
from tapa.cosim.verilator import generate_verilator_tb, launch_verilator
from tapa.cosim.vivado import get_vivado_tcl
from tapa.remote.config import get_remote_config
from tapa.remote.popen import create_tool_process

logging.root.handlers.clear()
logging.basicConfig(
    level=logging.INFO,
    format="[%(levelname)s:%(name)s:%(lineno)d] %(message)s",
    datefmt="%m%d %H:%M:%S",
)

_logger = logging.getLogger().getChild(__name__)


def _safe_eval_int(expr: str, params: dict[str, str]) -> int:
    """Evaluate a Verilog constant integer expression safely.

    Replaces parameter names with their values, then evaluates using
    ast.parse restricted to arithmetic operations only.
    """
    for name, val in params.items():
        expr = expr.replace(name, val)
    tree = _ast.parse(expr.strip(), mode="eval")
    for node in _ast.walk(tree):
        if not isinstance(
            node,
            (
                _ast.Expression,
                _ast.BinOp,
                _ast.UnaryOp,
                _ast.Constant,
                _ast.Add,
                _ast.Sub,
                _ast.Mult,
                _ast.FloorDiv,
                _ast.USub,
            ),
        ):
            msg = f"Unsafe expression: {expr!r}"
            raise ValueError(msg)
    return int(eval(compile(tree, "<string>", "eval")))


def parse_m_axi_interfaces(top_rtl_path: str) -> list[AXI]:
    """Parse the top RTL to extract all m_axi interface metadata."""
    with open(top_rtl_path, encoding="utf-8") as fp:
        top_rtl = fp.read()

    match_addr = re.findall(
        r"output\s+(?:wire\s+)?\[(.*):\s*0\s*\]\s+m_axi_(\w+)_ARADDR\s*[;,]", top_rtl
    )
    match_data = re.findall(
        r"output\s+(?:wire\s+)?\[(.*):\s*0\s*\]\s+m_axi_(\w+)_WDATA\s*[;,]", top_rtl
    )

    # the width may contain parameters
    params = re.findall(r"parameter\s+(\S+)\s*=\s+(\S+)\s*;", top_rtl)
    param_to_value = dict(params)

    axi_list = []
    name_to_addr_width = {m_axi: addr_width for addr_width, m_axi in match_addr}
    for data_width, m_axi in match_data:
        addr_width = name_to_addr_width[m_axi]
        axi_list.append(
            AXI(
                m_axi,
                _safe_eval_int(data_width, param_to_value) + 1,
                _safe_eval_int(addr_width, param_to_value) + 1,
            )
        )
    return axi_list


def get_cosim_tb(  # noqa: PLR0913,PLR0917
    top_name: str,
    top_is_leaf_task: bool,
    s_axi_control_path: str,
    axi_list: list[AXI],
    args: Sequence[Arg],
    scalar_to_val: dict[str, str],
    mode: str,
) -> str:
    """Generate a lightweight testbench to test the HLS RTL."""
    tb = get_begin() + "\n"

    for axi in axi_list:
        tb += get_axi_ram_inst(axi) + "\n"

    if mode == "vitis":
        arg_to_reg_addrs = parse_register_addr(s_axi_control_path)
        tb += get_s_axi_control() + "\n"
        tb += get_axis(args) + "\n"
        tb += get_vitis_dut(top_name, args) + "\n"
        tb += get_vitis_test_signals(arg_to_reg_addrs, scalar_to_val, args)
    else:
        tb += get_fifo(args) + "\n"
        tb += get_hls_dut(top_name, top_is_leaf_task, args, scalar_to_val) + "\n"
        tb += get_hls_test_signals(args)

    tb += get_end() + "\n"

    return tb


def set_default_nettype(verilog_path: str) -> None:
    """Appends `default_nettype` to Verilog files in the given directory.

    Sometimes the HLS-generated RTL will directly assign constants to IO ports But
    Vivado does not allow this behaviour. We need to set the `default_nettype to wire to
    bypass this issue.
    """
    _logger.debug("appending `default_nettype wire to every RTL file")
    for file in os.listdir(verilog_path):
        if file.endswith((".v", ".sv")):
            abs_path = os.path.join(verilog_path, file)
            with open(abs_path, "r+", encoding="utf-8") as f:
                content = f.read()
                f.seek(0, 0)
                f.write("`default_nettype wire\n" + content)


@click.command("cosim")
@click.option("--config-path", type=str, required=True)
@click.option("--tb-output-dir", type=str, required=True)
@click.option("--part-num", type=str)
@click.option("--launch-simulation / --no-launch-simulation", type=bool, default=False)
@click.option("--save-waveform / --no-save-waveform", type=bool, default=False)
@click.option("--start-gui / --no-start-gui", type=bool, default=False)
@click.option(
    "--simulator",
    type=click.Choice(["xsim", "verilator"]),
    default="xsim",
    help="Simulator backend to use. 'xsim' requires Vivado (Linux only). "
    "'verilator' is open-source and works on both Linux and macOS.",
)
def main(  # noqa: PLR0913, PLR0917
    config_path: str,
    tb_output_dir: str,
    part_num: str | None,
    launch_simulation: bool,
    save_waveform: bool,
    start_gui: bool,
    simulator: str,
) -> None:
    """Main entry point for the TAPA fast cosim tool."""
    _logger.info("TAPA fast cosim version: %s", __version__)
    _logger.debug(
        "config=%s tb_output_dir=%s part=%s sim=%s",
        config_path,
        tb_output_dir,
        part_num,
        simulator,
    )

    config = preprocess_config(config_path, tb_output_dir, part_num)

    top_name = config.top_name
    verilog_path = config.verilog_path
    top_path = f"{verilog_path}/{top_name}.v"
    ctrl_path = f"{verilog_path}/{top_name}_control_s_axi.v"

    axi_list = parse_m_axi_interfaces(top_path)

    if simulator == "verilator":
        generate_verilator_tb(config, axi_list, tb_output_dir)
        if launch_simulation:
            launch_verilator(config, tb_output_dir)
    else:
        _generate_xsim(
            config,
            top_name,
            verilog_path,
            ctrl_path,
            axi_list,
            tb_output_dir,
            save_waveform,
            start_gui,
            launch_simulation,
        )


def _generate_xsim(  # noqa: PLR0913, PLR0917
    config: CosimConfig,
    top_name: str,
    verilog_path: str,
    ctrl_path: str,
    axi_list: list[AXI],
    tb_output_dir: str,
    save_waveform: bool,
    start_gui: bool,
    launch_simulation: bool,
) -> None:
    """Generate xsim testbench and optionally launch Vivado simulation."""
    set_default_nettype(verilog_path)

    tb = get_cosim_tb(
        top_name,
        config.top_is_leaf_task,
        ctrl_path,
        axi_list,
        config.args,
        config.scalar_to_val,
        config.mode,
    )

    Path(tb_output_dir).mkdir(parents=True, exist_ok=True)
    for bin_file in Path(tb_output_dir).glob("*.bin"):
        bin_file.unlink()
    Path(f"{tb_output_dir}/tb.sv").write_text(tb, encoding="utf-8")
    Path(f"{tb_output_dir}/fifo_srl_tb.v").write_text(
        get_srl_fifo_template(), encoding="utf-8"
    )

    for axi in axi_list:
        ram_module = get_axi_ram_module(
            axi, config.axi_to_data_file[axi.name], config.axi_to_c_array_size[axi.name]
        )
        Path(f"{tb_output_dir}/axi_ram_{axi.name}.v").write_text(
            ram_module, encoding="utf-8"
        )

    Path(f"{tb_output_dir}/run").mkdir(parents=True, exist_ok=True)
    if save_waveform:
        _logger.warning(
            "Waveform will be saved at %s"
            "/run/vivado/tapa-fast-cosim/tapa-fast-cosim.sim"
            "/sim_1/behav/xsim/wave.wdb",
            tb_output_dir,
        )

    vivado_script = get_vivado_tcl(config, tb_output_dir, save_waveform, start_gui)
    Path(f"{tb_output_dir}/run/run_cosim.tcl").write_text(
        "\n".join(vivado_script), encoding="utf-8"
    )

    if launch_simulation:
        with (
            open(
                f"{tb_output_dir}/cosim.stdout.log", "w", encoding="utf-8"
            ) as stdout_fp,
            open(
                f"{tb_output_dir}/cosim.stderr.log", "w", encoding="utf-8"
            ) as stderr_fp,
        ):
            _launch_simulation(config, start_gui, tb_output_dir, stdout_fp, stderr_fp)


def _launch_simulation(
    config: CosimConfig,
    start_gui: bool,
    tb_output_dir: str,
    stdout_fp: TextIOWrapper,
    stderr_fp: TextIOWrapper,
) -> None:
    remote_config = get_remote_config()

    if start_gui and remote_config is not None:
        _logger.error("--start-gui is not supported with remote execution")
        sys.exit(1)

    mode = "gui" if start_gui else "batch"
    command = ["vivado", "-mode", mode, "-source", "run_cosim.tcl"]
    _logger.info("Starting Vivado: %s in %s/run", " ".join(command), tb_output_dir)

    run_dir = Path(f"{tb_output_dir}/run").resolve().as_posix()
    env = os.environ | {
        # Redirect Vivado's home directory to avoid collisions in the real home.
        "HOME": run_dir,
        "TAPA_FAST_COSIM_DPI_ARGS": ",".join(
            f"{k}:{v}" for k, v in config.axis_to_data_file.items()
        ),
    }

    if remote_config is not None:
        proc = create_tool_process(
            command,
            cwd=run_dir,
            env=env,
            extra_upload_paths=(run_dir,),
            extra_download_paths=(run_dir,),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        with proc:
            stdout_data, stderr_data = proc.communicate()
            stdout_fp.write(stdout_data.decode("utf-8", errors="replace"))
            stderr_fp.write(stderr_data.decode("utf-8", errors="replace"))
            if proc.returncode != 0:
                _logger.error(
                    "Vivado simulation failed with error code %d", proc.returncode
                )
                sys.exit(proc.returncode)
            _logger.info("Vivado simulation finished successfully")
    else:
        with subprocess.Popen(
            command,
            cwd=run_dir,
            env=env,
            stdout=stdout_fp,
            stderr=stderr_fp,
        ) as process:

            def kill_vivado_tree() -> None:
                """Kill the Vivado process and its children."""
                _logger.info("Killing Vivado process and its children")
                for child in psutil.Process(process.pid).children(recursive=True):
                    with suppress(psutil.NoSuchProcess):
                        child.kill()
                with suppress(psutil.NoSuchProcess):
                    process.kill()

            signal.signal(signal.SIGINT, lambda _s, _f: kill_vivado_tree())
            signal.signal(signal.SIGTERM, lambda _s, _f: kill_vivado_tree())

            process.wait()
            if process.returncode != 0:
                _logger.error(
                    "Vivado simulation failed with error code %d", process.returncode
                )
                sys.exit(process.returncode)
            _logger.info("Vivado simulation finished successfully")


if __name__ == "__main__":
    main()
