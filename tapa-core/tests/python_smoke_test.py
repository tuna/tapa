"""Smoke tests for tapa_core Python bindings."""

# ruff: noqa: PLC0415, PLR2004

from __future__ import annotations

import json
import os
import sys

# Allow running from repo root or tapa-core/
for _candidate in ["tapa-core/target/debug", "target/debug"]:
    if os.path.isdir(_candidate):
        sys.path.insert(0, _candidate)
        break


def _fixture_path(*parts: str) -> str:
    return os.path.join(os.path.dirname(__file__), "..", "testdata", *parts)


def _read_fixture(*parts: str) -> str:
    path = _fixture_path(*parts)
    with open(path, encoding="utf-8") as f:
        return f.read()


def test_import() -> None:
    import tapa_core  # type: ignore[import-not-found]

    assert hasattr(tapa_core, "protocol")
    assert hasattr(tapa_core, "task_graph")
    assert hasattr(tapa_core, "graphir")


def test_protocol_constants() -> None:
    from tapa_core import protocol  # type: ignore[import-not-found]

    assert protocol.HANDSHAKE_CLK == "ap_clk"
    assert protocol.M_AXI_PREFIX == "m_axi_"
    assert protocol.S_AXI_NAME == "s_axi_control"
    assert protocol.CLK_SENS_LIST == "posedge ap_clk"
    assert protocol.RTL_SUFFIX == ".v"
    assert isinstance(protocol.M_AXI_PORT_WIDTHS, dict)
    assert protocol.M_AXI_PORT_WIDTHS["ADDR"] == 0
    assert len(protocol.M_AXI_SUFFIXES) == 37
    assert len(protocol.M_AXI_PORTS) == 5
    assert len(protocol.M_AXI_SUFFIXES_BY_CHANNEL) == 5


def _deep_compare(py_val: object, rust_val: object, path: str) -> None:
    """Recursively compare Python and Rust constant values."""
    if isinstance(py_val, (list, tuple)) and isinstance(rust_val, (list, tuple)):
        py_t, rust_t = tuple(py_val), tuple(rust_val)
        assert len(py_t) == len(rust_t), f"{path}: length {len(py_t)} != {len(rust_t)}"
        for i, (pv, rv) in enumerate(zip(py_t, rust_t)):
            _deep_compare(pv, rv, f"{path}[{i}]")
    elif isinstance(py_val, dict) and isinstance(rust_val, dict):
        py_keys = set(py_val.keys())  # type: ignore[union-attr]
        rust_keys = set(rust_val.keys())  # type: ignore[union-attr]
        assert py_keys == rust_keys, f"{path}: keys {py_keys} != {rust_keys}"
        for k in py_val:  # type: ignore[union-attr]
            _deep_compare(py_val[k], rust_val[k], f"{path}[{k!r}]")  # type: ignore[index]
    else:
        assert py_val == rust_val, f"{path}: {py_val!r} != {rust_val!r}"


def test_protocol_parity() -> None:
    """Differential test: all 30 Python constants match Rust.

    Requires tapa.protocol to be importable (Bazel or editable install).
    Skipped if tapa.protocol is not available.
    """
    try:
        from tapa import protocol as py_proto
    except (ImportError, ModuleNotFoundError):
        return

    from tapa_core import protocol as rust_proto  # type: ignore[import-not-found]

    for name in py_proto.__all__:
        py_val = getattr(py_proto, name)
        assert hasattr(rust_proto, name), f"Rust missing: {name}"
        rust_val = getattr(rust_proto, name)
        _deep_compare(py_val, rust_val, name)


def test_task_graph_parse() -> None:
    from tapa_core import task_graph  # type: ignore[import-not-found]

    data = _read_fixture("task-graph", "vadd.json")
    result = task_graph.parse(data)
    assert result["top"] == "VecAdd"
    assert len(result["tasks"]) == 4


def test_task_graph_validate_good() -> None:
    from tapa_core import task_graph  # type: ignore[import-not-found]

    data = _read_fixture("task-graph", "vadd.json")
    task_graph.validate(data)


def test_task_graph_validate_bad() -> None:
    from tapa_core import task_graph  # type: ignore[import-not-found]

    try:
        task_graph.validate("")
    except ValueError:
        return
    msg = "should have raised ValueError"
    raise AssertionError(msg)


def test_task_graph_serialize() -> None:
    from tapa_core import task_graph  # type: ignore[import-not-found]

    data = _read_fixture("task-graph", "vadd.json")
    serialized = task_graph.serialize(data)
    parsed = json.loads(serialized)
    assert parsed["top"] == "VecAdd"


def test_graphir_parse() -> None:
    from tapa_core import graphir  # type: ignore[import-not-found]

    data = _read_fixture("graphir", "vadd_project.json")
    result = graphir.parse(data)
    assert result["modules"]["top_name"] == "VecAdd"


def test_graphir_serialize() -> None:
    from tapa_core import graphir  # type: ignore[import-not-found]

    data = _read_fixture("graphir", "vadd_project.json")
    serialized = graphir.serialize(data)
    parsed = json.loads(serialized)
    assert parsed["modules"]["top_name"] == "VecAdd"


_BAD_BASE64_JSON = (
    '{"modules": {"name": "$root", "module_definitions": []},'
    ' "blackboxes": [{"path": "x.v", "base64": "!!!bad!!!"}]}'
)

# Valid base64 wrapping non-zlib data (b"not-zlib" encoded).
_BAD_ZLIB_JSON = (
    '{"modules": {"name": "$root", "module_definitions": []},'
    ' "blackboxes": [{"path": "x.v", "base64": "bm90LXpsaWI="}]}'
)


def _assert_rejects_bad_blackbox(fn_name: str, payload: str) -> None:
    from tapa_core import graphir  # type: ignore[import-not-found]

    fn = getattr(graphir, fn_name)
    caught: ValueError | None = None
    try:
        fn(payload)
    except ValueError as e:
        caught = e
    assert caught is not None, f"graphir.{fn_name} should reject malformed blackbox"
    assert "blackboxes[0]" in str(caught), f"error has path: {caught}"


def test_graphir_parse_rejects_bad_base64() -> None:
    _assert_rejects_bad_blackbox("parse", _BAD_BASE64_JSON)


def test_graphir_validate_rejects_bad_base64() -> None:
    _assert_rejects_bad_blackbox("validate", _BAD_BASE64_JSON)


def test_graphir_serialize_rejects_bad_base64() -> None:
    _assert_rejects_bad_blackbox("serialize", _BAD_BASE64_JSON)


def test_graphir_parse_rejects_bad_zlib() -> None:
    _assert_rejects_bad_blackbox("parse", _BAD_ZLIB_JSON)


def test_graphir_validate_rejects_bad_zlib() -> None:
    _assert_rejects_bad_blackbox("validate", _BAD_ZLIB_JSON)


def test_graphir_serialize_rejects_bad_zlib() -> None:
    _assert_rejects_bad_blackbox("serialize", _BAD_ZLIB_JSON)


if __name__ == "__main__":
    _tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    for _t in _tests:
        _t()
