"""Tool process wrappers for Xilinx flows."""

from __future__ import annotations

import logging
import os
import shlex
import tempfile
from typing import TYPE_CHECKING, Self

if TYPE_CHECKING:
    from collections.abc import Iterable
    from types import TracebackType

from tapa.backend.kernel_metadata import M_AXI_PREFIX, S_AXI_NAME
from tapa.remote.config import get_remote_config
from tapa.remote.popen import create_tool_process

_logger = logging.getLogger().getChild(__name__)


class Vivado:
    """Call vivado with the given Tcl commands and arguments."""

    def __init__(self, commands: str, *args: Iterable[str]) -> None:
        self.cwd = tempfile.TemporaryDirectory(prefix="vivado-")
        with open(
            os.path.join(self.cwd.name, "commands.tcl"),
            mode="w+",
            encoding="locale",
        ) as tcl_file:
            tcl_file.write(commands)
        cmd_args = [
            "vivado",
            "-mode",
            "batch",
            "-source",
            tcl_file.name,
            "-nojournal",
            "-tclargs",
            *args,
        ]
        popen_kwargs: dict = {
            "env": os.environ
            | {
                "HOME": self.cwd.name,
            },
        }
        cmd_args = get_cmd_args(cmd_args, ["XILINX_VIVADO"], popen_kwargs)
        extra_upload = getattr(self, "_extra_upload", ())
        extra_download = getattr(self, "_extra_download", ())
        self._proc = create_tool_process(
            cmd_args,
            cwd=self.cwd.name,
            extra_upload_paths=extra_upload,
            extra_download_paths=extra_download,
            **popen_kwargs,
        )

    @property
    def returncode(self) -> int | None:
        return self._proc.returncode

    @returncode.setter
    def returncode(self, value: int | None) -> None:
        self._proc.returncode = value

    def communicate(self, timeout: float | None = None) -> tuple[bytes, bytes]:
        return self._proc.communicate(timeout=timeout)

    def __enter__(self) -> Self:
        self._proc.__enter__()
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self._proc.__exit__(exc_type, exc_value, traceback)
        self.cwd.cleanup()


class VivadoHls:
    """Call vivado_hls with the given Tcl commands."""

    def __init__(
        self,
        commands: str,
        hls: str = "vivado_hls",
        cwd: str = "",
        tclargs: tuple[str, ...] = (),
    ) -> None:
        if cwd:
            self.cwd = cwd
        else:
            self.cwd = tempfile.TemporaryDirectory(prefix=f"{hls}-")
            cwd = self.cwd.name
        with open(
            os.path.join(cwd, "commands.tcl"), mode="w+", encoding="locale"
        ) as tcl_file:
            tcl_file.write(commands)
        cmd_args_list: list[str] = [hls, "-f", tcl_file.name]
        if tclargs:
            cmd_args_list.extend(["-tclargs", *tclargs])
        extra_env = getattr(self, "_extra_env", {})
        popen_kwargs: dict = {
            "env": os.environ
            | {
                "HOME": cwd,
            }
            | extra_env,
        }
        cmd_args: list[str] | str = cmd_args_list
        if hls == "vitis_hls":
            cmd_args = get_cmd_args(
                cmd_args_list, ["XILINX_HLS", "XILINX_VITIS"], popen_kwargs
            )
        elif hls == "vivado_hls":
            cmd_args = get_cmd_args(cmd_args_list, ["XILINX_VIVADO"], popen_kwargs)
        extra_upload = getattr(self, "_extra_upload", ())
        extra_download = getattr(self, "_extra_download", ())
        self._proc = create_tool_process(
            cmd_args,
            cwd=cwd,
            extra_upload_paths=extra_upload,
            extra_download_paths=extra_download,
            **popen_kwargs,
        )

    @property
    def returncode(self) -> int | None:
        return self._proc.returncode

    @returncode.setter
    def returncode(self, value: int | None) -> None:
        self._proc.returncode = value

    def communicate(self, timeout: float | None = None) -> tuple[bytes, bytes]:
        return self._proc.communicate(timeout=timeout)

    def __enter__(self) -> Self:
        self._proc.__enter__()
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self._proc.__exit__(exc_type, exc_value, traceback)
        if isinstance(self.cwd, tempfile.TemporaryDirectory):
            self.cwd.cleanup()


PACKAGEXO_COMMANDS = r"""
# Paths passed via tclargs for remote execution path rewriting:
# argv[0] = tmpdir, argv[1] = hdl_dir, argv[2] = xo_file, argv[3] = kernel_xml
set tmpdir [lindex $argv 0]
set hdl_dir [lindex $argv 1]
set xo_file [lindex $argv 2]
set kernel_xml_path [lindex $argv 3]
set tmp_ip_dir "$tmpdir/tmp_ip_dir"
set tmp_project "$tmpdir/tmp_project"

create_project -force kernel_pack ${{tmp_project}}{part_num}
add_files [glob -nocomplain $hdl_dir/* $hdl_dir/*/* $hdl_dir/*/*/* \
        $hdl_dir/*/*/*/* $hdl_dir/*/*/*/*/*]
foreach tcl_file [glob -nocomplain $hdl_dir/*.tcl $hdl_dir/*/*.tcl] {{
  source ${{tcl_file}}
}}
set_property top {top_name} [current_fileset]
update_compile_order -fileset sources_1
update_compile_order -fileset sim_1
ipx::package_project -root_dir ${{tmp_ip_dir}} -vendor tapa \
        -library xrtl -taxonomy /KernelIP -import_files -set_current false
ipx::unload_core ${{tmp_ip_dir}}/component.xml
ipx::edit_ip_in_project -upgrade true -name tmp_edit_project \
        -directory ${{tmp_ip_dir}} ${{tmp_ip_dir}}/component.xml
set_property core_revision 2 [ipx::current_core]
foreach up [ipx::get_user_parameters] {{
  ipx::remove_user_parameter [get_property NAME ${{up}}] [ipx::current_core]
}}
set_property sdx_kernel true [ipx::current_core]
set_property sdx_kernel_type rtl [ipx::current_core]
ipx::create_xgui_files [ipx::current_core]
{bus_ifaces}
set_property xpm_libraries {{XPM_CDC XPM_MEMORY XPM_FIFO}} [ipx::current_core]
set_property supported_families {{ }} [ipx::current_core]
set_property auto_family_support_level level_2 [ipx::current_core]
ipx::update_checksums [ipx::current_core]
ipx::save_core [ipx::current_core]
close_project -delete

package_xo -force -xo_path "$xo_file" -kernel_name {top_name} \
        -ip_directory ${{tmp_ip_dir}} -kernel_xml $kernel_xml_path{cpp_kernels}
"""

BUS_IFACE = r"""
ipx::associate_bus_interfaces -busif {} -clock ap_clk [ipx::current_core]
"""

BUS_PARAM = """\
set_property value {2} [ipx::add_bus_parameter {1} [ipx::get_bus_interfaces {0}]]
"""


class PackageXo(Vivado):
    """Package the given files into a Xilinx hardware object."""

    def __init__(  # noqa: PLR0913,PLR0917
        self,
        xo_file: str,
        top_name: str,
        kernel_xml: str,
        hdl_dir: str,
        m_axi_names: Iterable[str] | dict[str, dict[str, str]] = (),
        iface_names: Iterable[str] = (S_AXI_NAME,),
        cpp_kernels: Iterable[str] = (),
        part_num: str = "",
    ) -> None:
        self.tmpdir = tempfile.TemporaryDirectory(prefix="package-xo-")
        self._xo_file = xo_file
        self._hdl_dir = hdl_dir
        self._kernel_xml = kernel_xml
        if _logger.isEnabledFor(logging.DEBUG):
            for _, _, files in os.walk(hdl_dir):
                for filename in files:
                    _logger.debug("packing: %s", filename)

        bus_ifaces: list[str] = list(map(BUS_IFACE.format, iface_names))
        for m_axi_name in m_axi_names:
            m_axi_iface_name = M_AXI_PREFIX + m_axi_name
            bus_ifaces.append(BUS_IFACE.format(m_axi_iface_name))
            if not isinstance(m_axi_names, dict):
                continue
            for key, value in m_axi_names.get(m_axi_name, {}).items():
                bus_ifaces.append(BUS_PARAM.format(m_axi_iface_name, key, value))

        kwargs = {
            "top_name": top_name,
            "bus_ifaces": "".join(bus_ifaces),
            "cpp_kernels": "".join(map(" -kernel_files {}".format, cpp_kernels)),
            "part_num": f" -part {part_num}" if part_num else "",
        }
        upload_paths: list[str] = [hdl_dir, self.tmpdir.name]
        if os.path.isfile(kernel_xml):
            upload_paths.append(kernel_xml)
        self._extra_upload = tuple(upload_paths)
        self._extra_download = (os.path.dirname(os.path.abspath(xo_file)),)
        super().__init__(
            PACKAGEXO_COMMANDS.format(**kwargs),
            self.tmpdir.name,
            hdl_dir,
            xo_file,
            kernel_xml,
        )

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        super().__exit__(exc_type, exc_value, traceback)
        self.tmpdir.cleanup()


def get_cmd_args(
    cmd_args: list[str],
    env_names: Iterable[str],
    kwargs: dict[str, str | int | bool],
) -> list[str] | str:
    """Get command arguments for tool process with specified environment."""
    if get_remote_config() is not None:
        return cmd_args

    for env_name in env_names:
        env_value = os.environ.get(env_name)
        if env_value is not None:
            settings = f"{env_value}/settings64.sh"
            if os.path.isfile(settings):
                kwargs["shell"] = True
                kwargs["executable"] = "bash"
                return " ".join(
                    [
                        "source",
                        shlex.quote(settings),
                        ";",
                        "exec",
                        *map(shlex.quote, cmd_args),
                    ]
                )
    return cmd_args
