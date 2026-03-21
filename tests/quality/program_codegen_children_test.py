"""Focused tests for extracted Program child-instantiation helpers."""

# ruff: noqa: ANN001, ANN401, ARG005, PLC2701, SLF001

from types import SimpleNamespace
from typing import Any, cast
from unittest.mock import Mock

from tapa.program_codegen import children
from tapa.program_codegen.children import _ChildState
from tapa.verilog.util import Pipeline


def _make_state(task: Any, program: Any, width_table: dict[str, int]) -> _ChildState:
    return _ChildState(
        program=program,
        task=task,
        width_table=width_table,
        is_done_signals=[],
        arg_table={},
        async_mmap_args={},
        fsm_upstream_portargs=[],
        fsm_upstream_module_ports={},
        fsm_downstream_portargs=[],
        fsm_downstream_module_ports=[],
    )


def test_collect_async_mmap_tags_matches_child_ports(monkeypatch) -> None:
    instance = cast("Any", SimpleNamespace())
    arg = cast(
        "Any",
        SimpleNamespace(
            cat=SimpleNamespace(is_async_mmap=True),
            port="port",
            mmap_name="mem",
            name="mem",
        ),
    )

    def fake_generate_async_mmap_ports(**kwargs: Any) -> list[SimpleNamespace]:
        if kwargs["tag"] == "axi":
            return [SimpleNamespace(portname="hit")]
        return []

    monkeypatch.setattr(
        children, "generate_async_mmap_ports", fake_generate_async_mmap_ports
    )

    tags = children._collect_async_mmap_tags(
        instance=instance,
        arg=arg,
        upper_name="mem_offset",
        offset_name="mem_offset",
        child_port_set={"hit"},
    )

    assert tags == ["axi"]


def test_bind_async_mmap_tag_uses_signal_path_for_upper_to_lower(monkeypatch) -> None:
    task_module = SimpleNamespace(add_signals=Mock(), add_ports=Mock())
    task = cast(
        "Any",
        SimpleNamespace(
            is_upper=True,
            module=task_module,
            fsm_module=SimpleNamespace(),
        ),
    )
    program = cast(
        "Any",
        SimpleNamespace(start_q=Pipeline("start"), done_q=Pipeline("done")),
    )
    state = _make_state(task=task, program=program, width_table={"mem": 32})
    instance = cast("Any", SimpleNamespace(task=SimpleNamespace(is_lower=True)))
    arg = cast("Any", SimpleNamespace(name="mem", mmap_name="mem"))

    monkeypatch.setattr(
        children, "generate_async_mmap_signals", lambda **kwargs: ["sig"]
    )
    monkeypatch.setattr(
        children, "generate_async_mmap_ioports", lambda **kwargs: ["io"]
    )

    children._bind_async_mmap_tag(state, instance, arg, "mem_offset", "axi")

    task_module.add_signals.assert_called_once_with(["sig"])
    task_module.add_ports.assert_not_called()


def test_declare_instance_start_logic_autorun_adds_fsm_logic(monkeypatch) -> None:
    task = cast(
        "Any",
        SimpleNamespace(
            fsm_module=SimpleNamespace(add_pipeline=Mock(), add_logics=Mock()),
        ),
    )
    program = cast(
        "Any",
        SimpleNamespace(start_q=Pipeline("start"), done_q=Pipeline("done")),
    )
    state = _make_state(task=task, program=program, width_table={})
    instance = cast(
        "Any",
        SimpleNamespace(
            start=SimpleNamespace(name="start"),
            is_autorun=True,
        ),
    )

    monkeypatch.setattr(
        children, "Always", lambda *args, **kwargs: ("Always", args, kwargs)
    )
    monkeypatch.setattr(
        children,
        "NonblockingSubstitution",
        lambda *args, **kwargs: ("NBS", args, kwargs),
    )
    monkeypatch.setattr(children, "make_block", lambda value: ("block", value))
    monkeypatch.setattr(children, "make_if_with_block", lambda **kwargs: ("if", kwargs))
    monkeypatch.setattr(children._CODEGEN, "visit", lambda value: value)

    children._declare_instance_start_logic(state, instance)

    task.fsm_module.add_pipeline.assert_called_once()
    task.fsm_module.add_logics.assert_called_once()
