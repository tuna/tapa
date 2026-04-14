import json
import logging
import os
from typing import Any

import click

from tapa.core import Program

_logger = logging.getLogger().getChild(__name__)

# ---------------------------------------------------------------------------
# design.json topology persistence
# ---------------------------------------------------------------------------

_DESIGN_CTX_NAME = "design"


def store_design(program: Program) -> None:
    """Serialize topology-only Program state to design.json."""
    design: dict[str, object] = {
        "top": program.top,
        "tasks": {
            name: task.to_topology_dict()
            for name, task in program._tasks.items()  # noqa: SLF001
        },
    }
    try:
        store_persistent_context(_DESIGN_CTX_NAME, design)
    except FileNotFoundError:
        _logger.debug("work directory does not exist; skipping design.json")


def load_design() -> dict:
    """Load topology-only Program state from design.json."""
    return load_persistent_context(_DESIGN_CTX_NAME)


def forward_applicable(
    ctx: click.Context,
    command: click.Command,
    kwargs: dict[str, Any],
) -> None:
    """Forward only applicable arguments to a subcommand."""
    names = {param.name for param in command.params}
    ctx.invoke(command, **{k: v for k, v in kwargs.items() if k in names})


def get_work_dir() -> str:
    """Returns the working directory of TAPA."""
    return click.get_current_context().obj["work-dir"]


def is_pipelined(step: str, pipelined: bool | None = None) -> bool | None:
    """Gets or sets if a step is pipelined in this single run."""
    if pipelined is None:
        return click.get_current_context().obj.get(f"{step}_pipelined", False)
    click.get_current_context().obj[f"{step}_pipelined"] = pipelined
    return None


def load_persistent_context(name: str) -> dict:
    """Try load context from the flow or from the workdir."""
    local_ctx = click.get_current_context().obj
    if local_ctx.get(name) is not None:
        _logger.info("reusing TAPA %s from upstream flows.", name)
        return local_ctx[name]

    json_file = os.path.join(get_work_dir(), f"{name}.json")
    _logger.info("loading TAPA graph from json `%s`.", json_file)
    try:
        with open(json_file, encoding="utf-8") as input_fp:
            local_ctx[name] = json.load(input_fp)
    except FileNotFoundError:
        msg = (
            f"Graph description {json_file} does not exist.  Either "
            "`tapa analyze` wasn't executed, or you specified a wrong path."
        )
        raise click.BadArgumentUsage(msg)
    return local_ctx[name]


def load_tapa_program() -> Program:
    """Try load program description from the flow or from the workdir."""
    local_ctx = click.get_current_context().obj
    if "tapa-program" not in local_ctx:
        local_ctx["tapa-program"] = Program(
            load_persistent_context("graph"),
            target=load_persistent_context("settings")["target"],
            work_dir=local_ctx["work-dir"],
        )
    return local_ctx["tapa-program"]


def store_persistent_context(name: str, ctx: dict | None = None) -> None:
    """Try store context to the workdir."""
    local_ctx = click.get_current_context().obj

    if ctx is not None:
        local_ctx[name] = ctx

    json_file = os.path.join(get_work_dir(), f"{name}.json")
    _logger.info("writing TAPA %s to json `%s`.", name, json_file)

    with open(json_file, "w", encoding="utf-8") as output_fp:
        json.dump(local_ctx[name], output_fp)


def store_tapa_program(prog: Program) -> None:
    """Store program description to the flow for downstream reuse."""
    click.get_current_context().obj["tapa-program"] = prog


def switch_work_dir(path: str) -> None:
    """Switch working directory to `path`."""
    os.makedirs(path, exist_ok=True)
    ctx = click.get_current_context().obj
    ctx["work-dir"] = path
    if "tapa-program" in ctx:
        ctx["tapa-program"].work_dir = path
