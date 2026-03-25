"""HLS and AIE execution wrappers for Xilinx flows."""

from __future__ import annotations

import glob
import logging
import os
import tarfile
import tempfile
from dataclasses import dataclass
from typing import TYPE_CHECKING, BinaryIO, Self

from tapa.backend.xilinx_tools import VivadoHls, get_cmd_args
from tapa.remote.popen import create_tool_process

if TYPE_CHECKING:
    from types import TracebackType

_logger = logging.getLogger().getChild(__name__)


@dataclass
class HlsConfig:
    top_name: str
    clock_period: str
    part_num: str
    reset_low: bool = True
    auto_prefix: bool = False
    hls: str = "vivado_hls"
    std: str = "c++11"
    other_configs: str = ""


def _build_kernel_env(
    kernel_files: list[str | tuple[str, str]],
    std: str,
) -> tuple[dict[str, str], tuple[str, ...]]:
    kernel_env: dict[str, str] = {"TAPA_KERNEL_COUNT": str(len(kernel_files))}
    upload_dirs: set[str] = set()
    for idx, kernel_file in enumerate(kernel_files):
        if isinstance(kernel_file, str):
            kernel_env[f"TAPA_KERNEL_PATH_{idx}"] = kernel_file
            kernel_env[f"TAPA_KERNEL_CFLAGS_{idx}"] = f"-std={std}"
            upload_dirs.add(os.path.dirname(os.path.abspath(kernel_file)))
        else:
            path, cflags = kernel_file
            kernel_env[f"TAPA_KERNEL_PATH_{idx}"] = path
            kernel_env[f"TAPA_KERNEL_CFLAGS_{idx}"] = f"-std={std} {cflags}"
            upload_dirs.add(os.path.dirname(os.path.abspath(path)))
            for part in cflags.split():
                if part.startswith("-isystem"):
                    inc_dir = os.path.abspath(part[len("-isystem") :])
                elif part.startswith("-I"):
                    inc_dir = os.path.abspath(part[2:])
                else:
                    continue
                if os.path.isdir(inc_dir):
                    upload_dirs.add(inc_dir)
    return kernel_env, tuple(upload_dirs)


def _build_rtl_config(hls: str, reset_low: bool, auto_prefix: bool) -> str:
    rtl_config = "config_rtl -reset_level " + ("low" if reset_low else "high")
    if auto_prefix:
        if hls == "vivado_hls":
            rtl_config += " -auto_prefix"
        elif hls == "vitis_hls":
            rtl_config += " -module_auto_prefix"
    return rtl_config


HLS_COMMANDS = r"""
cd [pwd]
open_project "{project_name}"
set_top {top_name}
for {{set i 0}} {{$i < $::env(TAPA_KERNEL_COUNT)}} {{incr i}} {{
    set kpath [set ::env(TAPA_KERNEL_PATH_$i)]
    set kcflags [set ::env(TAPA_KERNEL_CFLAGS_$i)]
    add_files "$kpath" -cflags "$kcflags"
}}
open_solution "{solution_name}"
set_part {{{part_num}}}
create_clock -period {clock_period} -name default
config_compile -name_max_length 253
config_interface -m_axi_addr64
{config}
{other_configs}
set_param hls.enable_hidden_option_error false
config_rtl -enableFreeRunPipeline=false
config_rtl -disableAutoFreeRunPipeline=true
csynth_design
exit
"""


class RunHls(VivadoHls):
    """Run Vivado/Vitis HLS for kernels and generate HDL files."""

    def __init__(
        self,
        tarfileobj: BinaryIO,
        kernel_files: list[str | tuple[str, str]],
        work_dir: str | None,
        config: HlsConfig,
    ) -> None:
        top_name = config.top_name
        if work_dir is None:
            self.tempdir = tempfile.TemporaryDirectory(prefix=f"run-hls-{top_name}-")
            self.project_path = self.tempdir.name
        else:
            self.tempdir = None
            self.project_path = f"{work_dir}/{top_name}"
            os.makedirs(self.project_path, exist_ok=True)
        self.project_name = "project"
        self.solution_name = top_name
        self.tarfileobj = tarfileobj
        self.hls = config.hls

        kernel_env, upload_dirs = _build_kernel_env(kernel_files, config.std)
        self._extra_upload = upload_dirs
        self._extra_download = (self.project_path,)
        self._extra_env = kernel_env

        rtl_config = _build_rtl_config(config.hls, config.reset_low, config.auto_prefix)
        kwargs = {
            "project_name": self.project_name,
            "solution_name": self.solution_name,
            "top_name": top_name,
            "part_num": config.part_num,
            "clock_period": config.clock_period,
            "config": rtl_config,
            "other_configs": config.other_configs,
        }
        super().__init__(
            HLS_COMMANDS.format(**kwargs),
            config.hls,
            self.project_path,
        )

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self._proc.__exit__(exc_type, exc_value, traceback)
        if self.returncode == 0:
            with tarfile.open(mode="w", fileobj=self.tarfileobj) as tar:
                solution_dir = os.path.join(
                    self.project_path, self.project_name, self.solution_name
                )
                try:
                    tar.add(
                        os.path.join(solution_dir, "syn/report"),
                        arcname=f"report/{self.solution_name}",
                    )
                    tar.add(os.path.join(solution_dir, "syn/report"), arcname="report")
                    tar.add(os.path.join(solution_dir, "syn/verilog"), arcname="hdl")
                    tar.add(
                        os.path.join(
                            solution_dir, self.project_path, f"{self.hls}.log"
                        ),
                        arcname="log/" + self.solution_name + ".log",
                    )
                    for pattern in (
                        "*.sched.adb.xml",
                        "*.verbose.sched.rpt",
                        "*.verbose.sched.rpt.xml",
                    ):
                        for file in glob.glob(
                            os.path.join(solution_dir, ".autopilot", "db", pattern)
                        ):
                            tar.add(
                                file,
                                arcname=os.path.join(
                                    "report",
                                    self.solution_name,
                                    os.path.basename(file),
                                ),
                            )
                            tar.add(file, arcname="report/" + os.path.basename(file))
                except FileNotFoundError as error:
                    self.returncode = 1
                    _logger.error("%s", error)
        if isinstance(self.cwd, tempfile.TemporaryDirectory):
            self.cwd.cleanup()
        if self.tempdir is not None:
            self.tempdir.cleanup()


class RunAie:
    """Run Vitis AIE for kernels and generate aie.a files."""

    def __init__(  # noqa: PLR0913,PLR0917
        self,
        tarfileobj: BinaryIO,
        kernel_files: list[str],
        work_dir: str | None,
        top_name: str,
        clock_period: str,  # noqa: ARG002
        xpfm: str | None,
    ) -> None:
        if work_dir is None:
            self.tempdir = tempfile.TemporaryDirectory(prefix=f"run-aie-{top_name}-")
            self.project_path = self.tempdir.name
        else:
            self.tempdir = None
            self.project_path = f"{work_dir}/{top_name}"
            os.makedirs(self.project_path, exist_ok=True)
        self.project_name = "project"
        self.solution_name = top_name
        self.tarfileobj = tarfileobj
        self.aiecompiler = "aiecompiler"
        self._extra_upload = tuple(
            {os.path.dirname(os.path.abspath(f)) for f in kernel_files}
        )
        popen_kwargs: dict = {"env": os.environ | {"HOME": self.project_path}}
        include_args = [f"--include={os.path.dirname(f)}" for f in kernel_files]
        cmd_args = get_cmd_args(
            [
                self.aiecompiler,
                "--target=hw",
                f"--platform={xpfm}",
                *include_args,
                f"--workdir={self.project_path}",
                *kernel_files,
            ],
            ["XILINX_VITIS"],
            popen_kwargs,
        )
        self._proc = create_tool_process(
            cmd_args,
            cwd=self.project_path,
            extra_upload_paths=self._extra_upload,
            extra_download_paths=(self.project_path,),
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
        if self.returncode == 0:
            with tarfile.open(mode="w", fileobj=self.tarfileobj) as tar:
                solution_dir = os.path.join(
                    self.project_path, self.project_name, self.solution_name
                )
                try:
                    tar.add(os.path.join(solution_dir, "syn/report"), arcname="report")
                    tar.add(os.path.join(solution_dir, "syn/verilog"), arcname="hdl")
                    tar.add(
                        os.path.join(
                            solution_dir, self.project_path, "AIECompiler.log"
                        ),
                        arcname="log/" + self.solution_name + ".log",
                    )
                    for pattern in (
                        "*.sched.adb.xml",
                        "*.verbose.sched.rpt",
                        "*.verbose.sched.rpt.xml",
                    ):
                        for file in glob.glob(
                            os.path.join(solution_dir, ".autopilot", "db", pattern)
                        ):
                            tar.add(file, arcname="report/" + os.path.basename(file))
                except FileNotFoundError as error:
                    self.returncode = 1
                    _logger.error("%s", error)
        if self.tempdir is not None:
            self.tempdir.cleanup()
