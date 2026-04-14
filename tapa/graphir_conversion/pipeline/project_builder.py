"""Project assembly helpers for GraphIR conversion."""

from __future__ import annotations

import json
from pathlib import Path
from typing import TYPE_CHECKING, cast

if TYPE_CHECKING:
    from collections.abc import Callable

    from tapa.core import Program
    from tapa.graphir.types import AnyModuleDefinition, Modules, Project
    from tapa.verilog.xilinx.module import Module


def get_island_to_pblock_range(
    device_config: Path,
    floorplan_path: Path,
) -> dict[str, list[str]]:
    """Load the floorplan-aware pblock mapping for a project."""
    with open(device_config, encoding="utf-8") as f:
        device = json.load(f)
    with open(floorplan_path, encoding="utf-8") as f:
        floorplan = json.load(f)

    slots = set(floorplan.values())
    slot_to_pblock_ranges = {}
    for slot in device["slots"]:
        x = slot["x"]
        y = slot["y"]
        slot_name = f"SLOT_X{x}Y{y}:SLOT_X{x}Y{y}"
        if slot_name not in slots:
            continue
        slot_to_pblock_ranges[slot_name.replace(":", "_TO_")] = slot["pblock_ranges"]
    return slot_to_pblock_ranges


def add_pblock_ranges(
    device_config: Path,
    project: Project,
    floorplan_path: Path,
) -> None:
    """Attach pblock ranges to a GraphIR project."""
    project.island_to_pblock_range = get_island_to_pblock_range(
        device_config=device_config,
        floorplan_path=floorplan_path,
    )


def _get_ctrl_s_axi_definition(
    program: Program,
    top_name: str,
    get_ctrl_s_axi_def: Callable,
) -> object:
    ctrl_s_axi_path = Path(program.rtl_dir) / f"{top_name}_control_s_axi.v"
    return get_ctrl_s_axi_def(
        program.top_task,
        ctrl_s_axi_path.read_text(encoding="utf-8"),
    )


def get_project_from_floorplanned_program(  # noqa: PLR0913
    program: Program,
    device_config: Path,
    floorplan_path: Path,
    *,
    get_verilog_module_from_leaf_task: Callable,
    get_slot_module_definition: Callable,
    get_top_module_definition: Callable,
    get_ctrl_s_axi_def: Callable,
    get_fsm_def: Callable,
    get_fifo_def: Callable,
    get_reset_inverter_def: Callable,
    get_graphir_iface: Callable,
    module_cls: type[Module],
    modules_cls: type[Modules],
    project_cls: type[Project],
) -> Project:
    """Assemble a GraphIR project from a floorplanned program."""
    top_task = program.top_task
    slot_tasks = {inst.task.name: inst.task for inst in top_task.instances}
    assert all(task.is_slot for task in slot_tasks.values())

    leaf_tasks = {
        inst.task.name: inst.task
        for slot_task in slot_tasks.values()
        for inst in slot_task.instances
    }

    leaf_irs = {}
    for task in leaf_tasks.values():
        task.module = module_cls(
            files=[Path(program.get_rtl_path(task.name))],
            is_trimming_enabled=False,
        )
        leaf_irs[task.name] = get_verilog_module_from_leaf_task(
            task, task.rtl_module.code
        )

    assert program.slot_task_name_to_fp_region is not None
    slot_irs = {
        task.name: get_slot_module_definition(
            task,
            leaf_irs,
            program.slot_task_name_to_fp_region[task.name],
        )
        for task in slot_tasks.values()
    }

    ctrl_s_axi = _get_ctrl_s_axi_definition(
        program,
        top_task.name,
        get_ctrl_s_axi_def,
    )
    top_ir = get_top_module_definition(
        top_task,
        slot_irs,
        ctrl_s_axi,
        program.slot_task_name_to_fp_region,
    )

    top_fsm_def = get_fsm_def(program.get_rtl_path(top_task.rtl_fsm_module.name))
    slot_fsms = [
        get_fsm_def(program.get_rtl_path(slot_task.rtl_fsm_module.name))
        for slot_task in slot_tasks.values()
    ]

    module_definitions = cast(
        "tuple[AnyModuleDefinition, ...]",
        (
            top_ir,
            ctrl_s_axi,
            top_fsm_def,
            get_fifo_def(),
            get_reset_inverter_def(),
            *slot_fsms,
            *slot_irs.values(),
            *leaf_irs.values(),
        ),
    )
    modules_obj = modules_cls(
        name="$root",
        module_definitions=module_definitions,
        top_name=top_task.name,
    )
    project = project_cls(modules=modules_obj)
    project.ifaces = get_graphir_iface(project, slot_tasks.values(), top_task)
    add_pblock_ranges(device_config, project, floorplan_path)
    return project
