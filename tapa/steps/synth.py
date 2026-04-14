"""Synthesize the TAPA program into RTL code."""

import json
import os
from pathlib import Path
from typing import NoReturn

import click

from tapa.abgraph.gen_abgraph import get_top_level_ab_graph
from tapa.backend.xilinx import parse_device_info
from tapa.common.target import Target
from tapa.core import Program
from tapa.graphir_conversion.gen_rs_graphir import get_project_from_floorplanned_program
from tapa.steps.common import (
    is_pipelined,
    load_persistent_context,
    load_tapa_program,
    store_design,
    store_persistent_context,
)
from tapa.steps.synth_plan import SynthPlan, build_synth_plan


@click.command()
@click.option(
    "--part-num",
    type=str,
    help="Target FPGA part number.  Must be specified if `--platform` is not provided.",
)
@click.option(
    "--platform",
    "-p",
    type=str,
    help="Target Vitis platform.  Must be specified if `--part-num` is not provided.",
)
@click.option("--clock-period", type=float, help="Target clock period in nanoseconds.")
@click.option(
    "--jobs",
    "-j",
    type=int,
    help="Number of parallel jobs for HLS (or RTL synthesis).",
)
@click.option(
    "--keep-hls-work-dir / --remove-hls-work-dir",
    type=bool,
    default=False,
    help="Keep HLS working directory in the TAPA work directory.",
)
@click.option(
    "--skip-hls-based-on-mtime / --no-skip-hls-based-on-mtime",
    type=bool,
    default=False,
    help=(
        "Skip HLS if an output tarball exists "
        "and is newer than the source C++ file. "
        "This can lead to incorrect results; use at your own risk."
    ),
)
@click.option(
    "--other-hls-configs",
    type=str,
    default="",
    help="Additional compile options for Vitis HLS, "
    'e.g., --other-hls-configs "config_compile -unsafe_math_optimizations"',
)
@click.option(
    "--enable-synth-util / --disable-synth-util",
    type=bool,
    default=False,
    help="Enable post-synthesis resource utilization report.",
)
@click.option(
    "--override-report-schema-version",
    type=str,
    default="",
    help="If non-empty, overrides the schema version in generated reports.",
)
@click.option(
    "--nonpipeline-fifos",
    type=click.Path(dir_okay=False, writable=True),
    default=None,
    help=(
        "Specifies the stream FIFOs to not add pipeline to. "
        "A `grouping_constraints.json` file will be generated for floorplanning."
    ),
)
@click.option(
    "--gen-ab-graph / --no-gen-ab-graph",
    type=bool,
    default=False,
    help="Specifies whether to generate the AutoBridge graph for partitioning.",
)
@click.option(
    "--gen-graphir",
    is_flag=True,
    help="Generate GraphIR for the TAPA program.",
)
@click.option(
    "--floorplan-config",
    type=Path,
    default=None,
    help="Path to the floorplan configuration file.",
)
@click.option(
    "--device-config",
    type=Path,
    default=None,
    help="Path to the device configuration file.",
)
@click.option(
    "--floorplan-path",
    type=Path,
    default=None,
    help="Path to the floorplan file. If specified, the floorplan will be applied.",
)
def synth(  # noqa: PLR0913,PLR0917
    part_num: str | None,
    platform: str | None,
    clock_period: float | str | None,
    jobs: int | None,
    keep_hls_work_dir: bool,
    skip_hls_based_on_mtime: bool,
    other_hls_configs: str,
    enable_synth_util: bool,
    override_report_schema_version: str,
    nonpipeline_fifos: Path | None,
    gen_ab_graph: bool,
    gen_graphir: bool,
    floorplan_config: Path | None,
    device_config: Path | None,
    floorplan_path: Path | None,
) -> None:
    """Synthesize the TAPA program into RTL code."""
    program = load_tapa_program()
    settings = load_persistent_context("settings")
    target = Target(settings.get("target"))

    # Automatically infer the information of the given device
    def on_error(msg: str) -> NoReturn:
        raise click.BadArgumentUsage(msg)

    device = parse_device_info(
        (platform, "--platform"),
        (part_num, "--part-num"),
        (clock_period, "--clock-period"),
        on_error,
    )
    part_num = device["part_num"]
    clock_period = device["clock_period"]

    # Save the context for downstream flows
    settings["part_num"] = part_num
    settings["platform"] = platform
    settings["clock_period"] = clock_period

    plan = build_synth_plan(
        target=target,
        part_num=part_num,
        platform=platform,
        clock_period=clock_period,
        jobs=jobs,
        keep_hls_work_dir=keep_hls_work_dir,
        skip_hls_based_on_mtime=skip_hls_based_on_mtime,
        other_hls_configs=other_hls_configs,
        enable_synth_util=enable_synth_util,
        override_report_schema_version=override_report_schema_version,
        nonpipeline_fifos=nonpipeline_fifos,
        gen_ab_graph=gen_ab_graph,
        gen_graphir=gen_graphir,
        floorplan_config=floorplan_config,
        device_config=device_config,
        floorplan_path=floorplan_path,
    )
    _execute_synth(program, plan, settings)


def _execute_synth(program: Program, plan: SynthPlan, settings: dict) -> None:
    """Execute the synth plan — all side effects live here."""
    assert plan.clock_period is not None
    assert isinstance(plan.clock_period, str)
    if plan.target == Target.XILINX_AIE:
        assert plan.platform is not None
        program.run_aie(
            plan.clock_period,
            plan.skip_hls_based_on_mtime,
            plan.keep_hls_work_dir,
            plan.platform,
        )
    elif plan.target in {Target.XILINX_VITIS, Target.XILINX_HLS}:
        assert plan.part_num is not None
        program.run_hls(
            plan.clock_period,
            plan.part_num,
            plan.skip_hls_based_on_mtime,
            plan.other_hls_configs,
            plan.jobs,
            plan.keep_hls_work_dir,
        )
        program.generate_task_rtl()
        if plan.enable_synth_util:
            program.generate_post_synth_util(plan.part_num, plan.jobs)
        program.generate_top_rtl(plan.override_report_schema_version)

        if plan.nonpipeline_fifos:
            with open(plan.nonpipeline_fifos, encoding="utf-8") as f:
                fifos = json.load(f)
            Path(
                os.path.join(program.work_dir, "grouping_constraints.json")
            ).write_text(
                json.dumps(program.get_grouping_constraints(fifos)), encoding="utf-8"
            )

        if plan.gen_ab_graph:
            assert plan.floorplan_config is not None
            Path(os.path.join(program.work_dir, "ab_graph.json")).write_text(
                get_top_level_ab_graph(
                    program, plan.floorplan_config
                ).model_dump_json(),
                encoding="utf-8",
            )

        if plan.gen_graphir:
            assert plan.device_config is not None
            assert plan.floorplan_path is not None
            Path(os.path.join(program.work_dir, "graphir.json")).write_text(
                get_project_from_floorplanned_program(
                    program, plan.device_config, plan.floorplan_path
                ).model_dump_json(),
                encoding="utf-8",
            )

        settings["synthed"] = True
        store_persistent_context("settings")
        store_persistent_context("templates_info", program.get_rtl_templates_info())
        store_design(program)

        is_pipelined("synth", True)
