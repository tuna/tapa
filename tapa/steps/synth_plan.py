"""Pure planning stage for the synth step: SynthPlan dataclass and builder."""

from dataclasses import dataclass
from pathlib import Path

from tapa.common.target import Target


@dataclass
class SynthPlan:
    """Immutable value object describing what synth should do."""

    target: Target
    part_num: str | None
    platform: str | None
    clock_period: float | str | None
    jobs: int | None
    keep_hls_work_dir: bool
    skip_hls_based_on_mtime: bool
    other_hls_configs: str
    enable_synth_util: bool
    override_report_schema_version: str
    nonpipeline_fifos: Path | None
    gen_ab_graph: bool
    gen_graphir: bool
    floorplan_config: Path | None
    device_config: Path | None
    floorplan_path: Path | None


def build_synth_plan(  # noqa: PLR0913,PLR0917
    target: Target,
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
) -> SynthPlan:
    """Validate inputs and return a SynthPlan. No side effects."""
    if target == Target.XILINX_AIE:
        assert platform is not None, "Platform must be specified for AIE flow."

    if gen_ab_graph:
        assert floorplan_config is not None, (
            "Floorplan configuration is required for generating AB graph."
        )

    if gen_graphir:
        assert device_config is not None, (
            "Device configuration is required for generating GraphIR."
        )
        assert floorplan_path is not None, (
            "Floorplan path is required for generating GraphIR."
        )

    return SynthPlan(
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
