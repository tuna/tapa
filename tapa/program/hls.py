"""HLS and AIE synthesis functionalities for TAPA programs."""

__copyright__ = """
Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

import itertools
import logging
import os
import os.path
import sys
from concurrent import futures
from pathlib import Path
from typing import Literal

from jinja2 import Environment, FileSystemLoader, StrictUndefined
from psutil import cpu_count

from tapa.backend.xilinx import RunAie, RunHls
from tapa.backend.xilinx_hls import HlsConfig
from tapa.common.paths import (
    find_resource,
    get_remote_hls_cflags,
    get_tapacc_cflags,
    get_xpfm_path,
)
from tapa.program.abc import ProgramInterface
from tapa.program.directory import ProgramDirectoryInterface
from tapa.program.hls_aie_codegen import (
    gen_connections,
    gen_declarations,
    gen_definitions,
)
from tapa.remote.config import get_remote_config
from tapa.safety_check import check_mmap_arg_name
from tapa.task import Task
from tapa.util import clang_format

_logger = logging.getLogger().getChild(__name__)

_env = Environment(
    loader=FileSystemLoader(str(Path(__file__).parent / "assets")),
    undefined=StrictUndefined,
    trim_blocks=True,
    lstrip_blocks=True,
)

# Maximum number of times a flaky Pre-synthesis failure will be retried.
_HLS_MAX_RETRIES = 1

# Re-export for tests that import _gen_connections from this module.
_gen_connections = gen_connections


class ProgramHlsMixin(
    ProgramDirectoryInterface,
    ProgramInterface,
):
    """Mixin class providing HLS (AIE included) functionalities."""

    top: str
    cflags: str
    _tasks: dict[str, Task]

    def _extract_cpp(self, target: Literal["aie", "hls"]) -> None:
        """Extract HLS/AIE C++ files."""
        _logger.info("extracting %s C++ files", target)
        check_mmap_arg_name(list(self._tasks.values()))

        top_aie_task_is_done = False
        for task in self._tasks.values():
            if task.name == self.top and target == "aie":
                assert not top_aie_task_is_done, (
                    "There should be exactly one top-level task"
                )
                top_aie_task_is_done = True
                code_content = self._get_aie_graph(task)
                with open(
                    self.get_header_path(task.name), "w", encoding="utf-8"
                ) as src_code:
                    src_code.write(code_content)
                with open(
                    self.get_cpp_path(task.name), "w", encoding="utf-8"
                ) as src_code:
                    src_code.write(
                        _env.get_template("aie_graph_cpp.j2").render(
                            top_task_name=self.top
                        )
                    )
                with open(self.get_common_path(), "w", encoding="utf-8") as src_code:
                    src_code.write(
                        clang_format(task.code).replace(
                            "#include <tapa.h>", "#include <adf.h>"
                        )
                    )
            else:
                code_content = clang_format(task.code)
                if target == "aie":
                    code_content = code_content.replace(
                        "#include <tapa.h>", "#include <adf.h>"
                    )
                try:
                    with open(
                        self.get_cpp_path(task.name), encoding="utf-8"
                    ) as src_code:
                        if src_code.read() == code_content:
                            _logger.debug(
                                "not updating %s since its content is up-to-date",
                                src_code.name,
                            )
                            continue
                except FileNotFoundError:
                    pass
                with open(
                    self.get_cpp_path(task.name), "w", encoding="utf-8"
                ) as src_code:
                    src_code.write(code_content)

    def _is_skippable_based_on_mtime(self, task_name: str) -> bool:
        try:
            tar_path = self.get_tar_path(task_name)
            cpp_path = self.get_cpp_path(task_name)
            if os.path.getmtime(tar_path) > os.path.getmtime(cpp_path):
                _logger.info(
                    "skipping HLS for %s since %s is newer than %s",
                    task_name,
                    tar_path,
                    cpp_path,
                )
                return True
        except OSError:
            pass
        return False

    def _build_hls_cflags(self) -> str:
        """Build HLS compiler flags, substituting remote-friendly headers if needed."""
        hls_defines = "-DTAPA_TARGET_DEVICE_ -DTAPA_TARGET_XILINX_HLS_"
        # WORKAROUND: Vitis HLS requires -I or gflags cannot be found...
        try:
            hls_includes = f"-I{find_resource('tapa-extra-runtime-include')}"
        except FileNotFoundError:
            hls_includes = ""
        # For remote HLS, use only TAPA-specific headers (not local vendor/
        # stdlib paths). The remote Vitis HLS handles vendor headers natively
        # via settings64.sh, so uploading local Vitis copies is unnecessary
        # and can cause header conflicts.
        if get_remote_config() is not None:
            local_suffix = " ".join(get_tapacc_cflags())
            remote_suffix = " ".join(get_remote_hls_cflags())
            base_cflags = self.cflags.replace(local_suffix, remote_suffix)
        else:
            base_cflags = self.cflags
        return f"{base_cflags} {hls_defines} {hls_includes}"

    def _run_hls_task(  # noqa: PLR0913, PLR0917
        self,
        task: Task,
        hls_cflags: str,
        clock_period: str,
        part_num: str,
        other_configs: str,
        work_dir: str | None,
    ) -> None:
        """Run HLS for a single task with retry on flaky Pre-synthesis failures."""
        for attempt in range(_HLS_MAX_RETRIES + 1):
            with (
                open(self.get_tar_path(task.name), "wb") as tarfileobj,
                RunHls(
                    tarfileobj,
                    kernel_files=[(self.get_cpp_path(task.name), hls_cflags)],
                    work_dir=work_dir,
                    config=HlsConfig(
                        top_name=task.name,
                        clock_period=clock_period,
                        part_num=part_num,
                        auto_prefix=True,
                        hls="vitis_hls",
                        std="c++14",
                        other_configs=other_configs,
                    ),
                ) as proc,
            ):
                stdout, stderr = proc.communicate()

            if proc.returncode == 0:
                return

            if (
                b"Pre-synthesis failed." in stdout
                and b"\nERROR:" not in stdout
                and attempt < _HLS_MAX_RETRIES
            ):
                _logger.error(
                    "HLS failed for %s, but the failure may be flaky; retrying",
                    task.name,
                )
                continue

            sys.stdout.write(stdout.decode("utf-8"))
            sys.stderr.write(stderr.decode("utf-8"))
            msg = f"HLS failed for {task.name}"
            raise RuntimeError(msg)

    def run_hls(  # noqa: PLR0913, PLR0917
        self,
        clock_period: str,
        part_num: str,
        skip_based_on_mtime: bool,
        other_configs: str,
        jobs: int | None,
        keep_hls_work_dir: bool,
    ) -> None:
        """Run HLS with extracted HLS C++ files and generate tarballs."""
        self._extract_cpp("hls")
        _logger.info("running hls")
        work_dir = os.path.join(self.work_dir, "hls") if keep_hls_work_dir else None
        hls_cflags = self._build_hls_cflags()

        def worker(task: Task, idx: int) -> None:
            _logger.info("start worker for %s, target: %s", task.name, task.target_type)
            os.nice(idx % 19)
            if skip_based_on_mtime and self._is_skippable_based_on_mtime(task.name):
                return
            self._run_hls_task(
                task, hls_cflags, clock_period, part_num, other_configs, work_dir
            )

        jobs = jobs or cpu_count(logical=False)
        _logger.info("spawn %d workers for parallel HLS synthesis of the tasks", jobs)

        try:
            with futures.ThreadPoolExecutor(max_workers=jobs) as executor:
                any(executor.map(worker, self._tasks.values(), itertools.count()))
        except RuntimeError:
            if keep_hls_work_dir:
                _logger.error(
                    "HLS failed, see above for details. You may use "
                    "`--keep-hls-work-dir` to keep the HLS work directory "
                    "for debugging."
                )
            else:
                _logger.error(
                    "HLS failed, see above for details. Please check the logs in %s",
                    work_dir,
                )
            sys.exit(1)

    def run_aie(
        self,
        clock_period: str,
        skip_based_on_mtime: bool,
        keep_hls_work_dir: bool,
        platform: str,
    ) -> None:
        """Run HLS with extracted HLS C++ files and generate tarballs."""
        self._extract_cpp("aie")

        _logger.info("running aie")
        work_dir = os.path.join(self.work_dir, "aie") if keep_hls_work_dir else None

        # For AIE flow, only the top-level task is synthesized
        task = self.top_task
        if skip_based_on_mtime and self._is_skippable_based_on_mtime(task.name):
            return
        with (
            open(self.get_tar_path(task.name), "wb") as tarfileobj,
            RunAie(
                tarfileobj,
                kernel_files=[self.get_cpp_path(task.name)],
                work_dir=work_dir,
                top_name=task.name,
                clock_period=clock_period,
                xpfm=get_xpfm_path(platform),
            ) as proc,
        ):
            stdout, stderr = proc.communicate()

        if proc.returncode != 0:
            sys.stdout.write(stdout.decode("utf-8"))
            sys.stderr.write(stderr.decode("utf-8"))

            # Neglect the dummy bug message from AIE 2022.2
            aie_dummy_bug_msg = "/bin/sh: 1: [[: not found"
            if aie_dummy_bug_msg in stderr.decode("utf-8"):
                return

            if work_dir is None:
                _logger.error(
                    "HLS failed, see above for details. You may use "
                    "`--keep-hls-work-dir` to keep the HLS work directory "
                    "for debugging."
                )
            else:
                _logger.error(
                    "HLS failed, see above for details. Please check the logs in %s",
                    work_dir,
                )
            sys.exit(1)

    def _get_aie_graph(self, task: Task) -> str:
        """Generates the complete AIE graph code."""
        _, kernel_decl, port_decl = gen_declarations(task)
        kernel_def, kernel_source, kernel_runtime, kernel_loc, port_def = (
            gen_definitions(task)
        )
        connect_def = gen_connections(task)

        return _env.get_template("aie_graph_header.j2").render(
            graph_name=self.top,
            kernel_decl="\n\t".join(kernel_decl),
            kernel_def="\n\t\t".join(kernel_def),
            kernel_source="\n\t\t".join(kernel_source),
            kernel_header="",
            kernel_runtime="\n\t\t".join(kernel_runtime),
            kernel_loc="\n\t\t".join(kernel_loc),
            port_decl="\n\t".join(port_decl),
            port_def="\n\t\t".join(port_def),
            connect_def="\n\t\t".join(connect_def),
        )
