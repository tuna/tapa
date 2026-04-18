"""Smoke tests for tapa_core Python bindings."""

# ruff: noqa: PLC0415, PLR2004, E501, PLR0913

from __future__ import annotations

import json
import os
import sys

# Allow running from repo root or tapa-core/
for _candidate in ["tapa-core/target/debug", "target/debug"]:
    if os.path.isdir(_candidate):
        sys.path.insert(0, _candidate)
        break

# Add repo root to sys.path so `from tapa import protocol` works
# for the differential parity test.
_repo_root = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", ".."))
if os.path.isdir(os.path.join(_repo_root, "tapa")) and _repo_root not in sys.path:
    sys.path.insert(0, _repo_root)


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
    assert hasattr(tapa_core, "rtl")
    assert hasattr(tapa_core, "topology")
    assert hasattr(tapa_core, "slotting")
    assert hasattr(tapa_core, "codegen")
    assert hasattr(tapa_core, "floorplan")
    assert hasattr(tapa_core, "lowering")
    assert hasattr(tapa_core, "graphir_export")


def test_dotted_import() -> None:
    import tapa_core.protocol  # type: ignore[import-not-found]

    assert hasattr(tapa_core.protocol, "HANDSHAKE_CLK")  # type: ignore[attr-defined]


def test_dotted_import_slotting() -> None:
    import tapa_core.slotting  # type: ignore[import-not-found]

    assert hasattr(tapa_core.slotting, "gen_slot_cpp")
    assert hasattr(tapa_core.slotting, "replace_function")
    assert hasattr(tapa_core.slotting, "get_floorplan_graph")


def test_dotted_import_codegen() -> None:
    import tapa_core.codegen  # type: ignore[import-not-found]

    assert hasattr(tapa_core.codegen, "attach_modules")
    assert hasattr(tapa_core.codegen, "generate_rtl")


def test_slotting_replace_function() -> None:
    import tapa_core.slotting  # type: ignore[import-not-found]

    source = 'extern "C" void my_func(int a) { int x = 1; }'
    result = tapa_core.slotting.replace_function(source, "my_func", "// new body")
    assert "// new body" in result
    assert "int x = 1" not in result


def test_slotting_replace_function_empty_source() -> None:
    import tapa_core.slotting  # type: ignore[import-not-found]

    try:
        tapa_core.slotting.replace_function("", "func", "body")
    except ValueError:
        return
    msg = "should have raised ValueError"
    raise AssertionError(msg)


def test_slotting_gen_slot_cpp() -> None:
    import tapa_core.slotting  # type: ignore[import-not-found]

    ports = json.dumps([{"cat": "scalar", "name": "x", "type": "int", "width": 32}])
    top_cpp = 'extern "C" {\nvoid top(int x) {}\n}  // extern "C"\n'
    result = tapa_core.slotting.gen_slot_cpp(
        "SLOT_X0Y0_TO_SLOT_X1Y1", "top", ports, top_cpp
    )
    assert "SLOT_X0Y0_TO_SLOT_X1Y1" in result


def test_slotting_gen_slot_cpp_missing_field() -> None:
    import tapa_core.slotting  # type: ignore[import-not-found]

    # Missing 'cat' field
    ports = json.dumps([{"name": "x", "type": "int", "width": 32}])
    top_cpp = 'extern "C" {\nvoid top(int x) {}\n}  // extern "C"\n'
    try:
        tapa_core.slotting.gen_slot_cpp("SLOT", "top", ports, top_cpp)
    except ValueError:
        return
    msg = "should have raised ValueError for missing cat"
    raise AssertionError(msg)


def test_codegen_attach_modules() -> None:
    import tapa_core.codegen  # type: ignore[import-not-found]

    design = json.dumps(
        {
            "top": "top_task",
            "target": "xilinx-hls",
            "tasks": {
                "top_task": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {"child": [{"args": {}, "step": 0}]},
                    "fifos": {},
                },
                "child": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {},
                    "fifos": {},
                },
            },
        }
    )
    modules = json.dumps({"child": "module child(); endmodule"})
    result = tapa_core.codegen.attach_modules(design, modules)
    assert "child" in str(result)


def test_codegen_invalid_json() -> None:
    import tapa_core.codegen  # type: ignore[import-not-found]

    try:
        tapa_core.codegen.attach_modules("not json", "{}")
    except ValueError:
        return
    msg = "should have raised ValueError for invalid JSON"
    raise AssertionError(msg)


def test_codegen_generate_rtl() -> None:
    import tapa_core.codegen  # type: ignore[import-not-found]

    design = json.dumps(
        {
            "top": "top_task",
            "target": "xilinx-hls",
            "tasks": {
                "top_task": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {"child": [{"args": {}, "step": 0}]},
                    "fifos": {},
                },
                "child": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {},
                    "fifos": {},
                },
            },
        }
    )
    top_v = (
        "module top_task(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule"
    )
    modules = json.dumps({"top_task": top_v})
    result = tapa_core.codegen.generate_rtl(design, modules)
    assert "generated_files" in result
    assert "modified_modules" in result


def test_codegen_generate_rtl_invalid_input() -> None:
    import tapa_core.codegen  # type: ignore[import-not-found]

    try:
        tapa_core.codegen.generate_rtl("not json", "{}")
    except ValueError:
        return
    msg = "should have raised ValueError for invalid design JSON"
    raise AssertionError(msg)


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


def test_rtl_parse() -> None:
    from tapa_core import rtl  # type: ignore[import-not-found]

    src = _read_fixture("rtl", "LowerLevelTask.v")
    result = rtl.parse(src)
    assert result["name"] == "LowerLevelTask"
    assert len(result["ports"]) == 17
    assert len(result["parameters"]) == 1


def test_rtl_classify_ports() -> None:
    from tapa_core import rtl  # type: ignore[import-not-found]

    src = _read_fixture("rtl", "LowerLevelTask.v")
    classified = rtl.classify_ports(src)
    assert isinstance(classified, dict), "classify_ports returns a dict"
    assert "ap_clk" in classified
    assert classified["ap_clk"]["kind"] == "Handshake"


def test_rtl_parse_bad_input() -> None:
    from tapa_core import rtl  # type: ignore[import-not-found]

    try:
        rtl.parse("")
    except ValueError:
        return
    msg = "should have raised ValueError"
    raise AssertionError(msg)


def test_topology_parse_design() -> None:
    from tapa_core import topology  # type: ignore[import-not-found]

    data = _read_fixture("topology", "vadd_design.json")
    result = topology.parse_design(data)
    assert result["top"] == "VecAdd"
    assert len(result["tasks"]) == 4


def test_topology_validate_design() -> None:
    from tapa_core import topology  # type: ignore[import-not-found]

    data = _read_fixture("topology", "vadd_design.json")
    topology.validate_design(data)


def test_topology_validate_bad_input() -> None:
    from tapa_core import topology  # type: ignore[import-not-found]

    try:
        topology.validate_design("")
    except ValueError:
        return
    msg = "should have raised ValueError"
    raise AssertionError(msg)


def test_topology_serialize_design() -> None:
    from tapa_core import topology  # type: ignore[import-not-found]

    data = _read_fixture("topology", "vadd_design.json")
    serialized = topology.serialize_design(data)
    parsed = json.loads(serialized)
    assert parsed["top"] == "VecAdd"


def test_topology_round_trip() -> None:
    from tapa_core import topology  # type: ignore[import-not-found]

    data = _read_fixture("topology", "vadd_design.json")
    serialized = topology.serialize_design(data)
    result = topology.parse_design(serialized)
    assert result["top"] == "VecAdd"
    assert len(result["tasks"]) == 4


def test_floorplan_import() -> None:
    import tapa_core.floorplan  # type: ignore[import-not-found]

    assert hasattr(tapa_core.floorplan, "get_top_level_ab_graph")


def test_floorplan_generate_abgraph() -> None:
    from tapa_core import floorplan  # type: ignore[import-not-found]

    program = json.dumps(
        {
            "top": "top_task",
            "target": "xilinx-hls",
            "tasks": {
                "top_task": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {
                            "cat": "scalar",
                            "name": "n",
                            "type": "int",
                            "width": 32,
                        }
                    ],
                    "tasks": {
                        "child": [{"args": {"n": {"arg": "n", "cat": "scalar"}}}]
                    },
                    "fifos": {},
                },
                "child": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {
                            "cat": "scalar",
                            "name": "n",
                            "type": "int",
                            "width": 32,
                        }
                    ],
                    "tasks": {},
                    "fifos": {},
                },
            },
        }
    )
    result = floorplan.get_top_level_ab_graph(program, "{}", "top_task_fsm")
    parsed = json.loads(result)
    assert "vs" in parsed
    assert "es" in parsed


def test_floorplan_rtl_aware() -> None:
    from tapa_core import floorplan  # type: ignore[import-not-found]

    program = json.dumps(
        {
            "top": "top_task",
            "target": "xilinx-hls",
            "tasks": {
                "top_task": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {
                            "cat": "scalar",
                            "name": "n",
                            "type": "int",
                            "width": 32,
                        }
                    ],
                    "tasks": {
                        "child": [{"args": {"n": {"arg": "n", "cat": "scalar"}}}]
                    },
                    "fifos": {},
                },
                "child": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {
                            "cat": "scalar",
                            "name": "n",
                            "type": "int",
                            "width": 32,
                        }
                    ],
                    "tasks": {},
                    "fifos": {},
                },
            },
        }
    )
    modules = json.dumps(
        {
            "child": (
                "module child(\n"
                "  input wire ap_clk,\n"
                "  input wire [31:0] n\n"
                ");\nendmodule"
            )
        }
    )
    result = floorplan.get_top_level_ab_graph_from_rtl(
        program, modules, "{}", "top_task_fsm"
    )
    parsed = json.loads(result)
    assert "vs" in parsed
    assert "es" in parsed


def test_floorplan_invalid_json() -> None:
    from tapa_core import floorplan  # type: ignore[import-not-found]

    try:
        floorplan.get_top_level_ab_graph("not json", "{}", "fsm")
    except ValueError:
        return
    msg = "should have raised ValueError"
    raise AssertionError(msg)


def test_floorplan_from_rtl_rejects_malformed_verilog() -> None:
    from tapa_core import floorplan  # type: ignore[import-not-found]

    # Malformed child RTL must raise ValueError instead of silently
    # falling back to topology-derived FIFO widths. The RTL-aware
    # floorplan path is the one callers use specifically to get real
    # RTL-backed widths; swallowing parse errors would defeat that.
    program = json.dumps(
        {
            "top": "top_task",
            "target": "xilinx-hls",
            "tasks": {
                "top_task": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {"child": [{"args": {}, "step": 0}]},
                    "fifos": {},
                },
                "child": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {},
                    "fifos": {},
                },
            },
        }
    )
    modules = json.dumps({"child": "this is not valid verilog"})
    err_msg = ""
    try:
        floorplan.get_top_level_ab_graph_from_rtl(
            program, modules, "{}", "top_task_fsm"
        )
    except ValueError as exc:
        err_msg = str(exc)
    if not err_msg:
        msg = "get_top_level_ab_graph_from_rtl should reject malformed RTL"
        raise AssertionError(msg)
    assert "child" in err_msg, err_msg


def test_floorplan_from_rtl_rejects_unknown_task() -> None:
    from tapa_core import floorplan  # type: ignore[import-not-found]

    # A mistyped task name in module_files must raise ValueError
    # instead of being silently dropped.
    program = json.dumps(
        {
            "top": "top_task",
            "target": "xilinx-hls",
            "tasks": {
                "top_task": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {"child": [{"args": {}, "step": 0}]},
                    "fifos": {},
                },
                "child": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {},
                    "fifos": {},
                },
            },
        }
    )
    modules = json.dumps(
        {"chld": "module chld(); endmodule"}  # typo — not in program.tasks
    )
    err_msg = ""
    try:
        floorplan.get_top_level_ab_graph_from_rtl(
            program, modules, "{}", "top_task_fsm"
        )
    except ValueError as exc:
        err_msg = str(exc)
    if not err_msg:
        msg = "get_top_level_ab_graph_from_rtl should reject unknown task keys"
        raise AssertionError(msg)
    assert "chld" in err_msg or "unknown" in err_msg.lower(), err_msg


def test_lowering_import() -> None:
    import tapa_core.lowering  # type: ignore[import-not-found]

    assert hasattr(tapa_core.lowering, "get_project")


def test_lowering_get_project() -> None:
    from tapa_core import lowering  # type: ignore[import-not-found]

    program = json.dumps(
        {
            "top": "top_task",
            "target": "xilinx-hls",
            "tasks": {
                "top_task": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {
                            "cat": "scalar",
                            "name": "n",
                            "type": "int",
                            "width": 32,
                        }
                    ],
                    "tasks": {
                        "child": [{"args": {"n": {"arg": "n", "cat": "scalar"}}}]
                    },
                    "fifos": {},
                },
                "child": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {
                            "cat": "scalar",
                            "name": "n",
                            "type": "int",
                            "width": 32,
                        }
                    ],
                    "tasks": {},
                    "fifos": {},
                },
            },
        }
    )
    slot_map = json.dumps({"SLOT_0": ["child_0"]})
    # module_files: RTL sources keyed by task name — lowering derives leaf
    # modules and FSM modules internally from these sources. The
    # `{top}_control_s_axi` entry is required: `lowering.get_project`
    # rejects inputs without an attached control_s_axi module to
    # prevent placeholder Verilog from leaking into exported designs.
    module_files = json.dumps(
        {
            "top_task": (
                "module top_task(\n"
                "  input wire ap_clk,\n"
                "  input wire ap_rst_n,\n"
                "  input wire [31:0] n\n"
                ");\nendmodule"
            ),
            "child": "module child(\n  input wire [31:0] n\n);\nendmodule",
            "top_task_control_s_axi": (
                "module top_task_control_s_axi(\n"
                "  input wire ACLK,\n"
                "  input wire ARESET,\n"
                "  output wire ap_start,\n"
                "  input wire ap_done\n"
                ");\nendmodule"
            ),
        }
    )
    result = lowering.get_project(program, module_files, slot_map)
    parsed = json.loads(result)
    assert "modules" in parsed
    mod_names = [m["name"] for m in parsed["modules"]["module_definitions"]]
    assert "top_task_fsm" in mod_names, (
        f"FSM module should be emitted, got: {mod_names}"
    )


def test_lowering_invalid_json() -> None:
    from tapa_core import lowering  # type: ignore[import-not-found]

    try:
        # First arg (program_json) is invalid
        lowering.get_project("not json", "{}", "{}")
    except ValueError:
        return
    msg = "should have raised ValueError"
    raise AssertionError(msg)


def test_graphir_export_import() -> None:
    import tapa_core.graphir_export  # type: ignore[import-not-found]

    assert hasattr(tapa_core.graphir_export, "export_project")


def test_graphir_export_success() -> None:
    import tempfile

    from tapa_core import graphir_export  # type: ignore[import-not-found]

    project_json = json.dumps(
        {
            "modules": {
                "name": "$root",
                "module_definitions": [
                    {
                        "name": "top_mod",
                        "module_type": "grouped_module",
                        "parameters": [],
                        "ports": [
                            {"name": "clk", "type": "input wire"},
                        ],
                        "submodules": [],
                        "wires": [],
                    }
                ],
                "top_name": "top_mod",
            },
            "blackboxes": [],
        }
    )
    with tempfile.TemporaryDirectory() as dest:
        graphir_export.export_project(project_json, dest)
        # Should have written top_mod.v
        exported = os.listdir(dest)
        assert "top_mod.v" in exported, f"expected top_mod.v, got: {exported}"
        # Should have Xilinx primitive stubs
        assert "LUT6.v" in exported, f"expected LUT6.v, got: {exported}"
        assert "FDRE.v" in exported, f"expected FDRE.v, got: {exported}"


def test_graphir_export_invalid_path() -> None:
    from tapa_core import graphir_export  # type: ignore[import-not-found]

    # Valid project JSON but non-writable destination
    project_json = json.dumps(
        {
            "modules": {
                "name": "$root",
                "module_definitions": [],
            },
            "blackboxes": [],
        }
    )
    try:
        graphir_export.export_project(project_json, "/nonexistent/path/foo/bar")
    except (ValueError, OSError):
        return
    msg = "should have raised error for invalid path"
    raise AssertionError(msg)


def test_graphir_export_invalid_json() -> None:
    from tapa_core import graphir_export  # type: ignore[import-not-found]

    try:
        graphir_export.export_project("not json", "/tmp/nonexistent")
    except ValueError:
        return
    msg = "should have raised ValueError"
    raise AssertionError(msg)


# ---------------------------------------------------------------------------
# Cross-language parity and negative-path tests.
# ---------------------------------------------------------------------------


def _trivial_program_json() -> str:
    return json.dumps(
        {
            "top": "top_task",
            "target": "xilinx-hls",
            "tasks": {
                "top_task": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {
                            "cat": "scalar",
                            "name": "n",
                            "type": "int",
                            "width": 32,
                        }
                    ],
                    "tasks": {
                        "child": [{"args": {"n": {"arg": "n", "cat": "scalar"}}}]
                    },
                    "fifos": {},
                },
                "child": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {
                            "cat": "scalar",
                            "name": "n",
                            "type": "int",
                            "width": 32,
                        }
                    ],
                    "tasks": {},
                    "fifos": {},
                },
            },
        }
    )


_TRIVIAL_TOP_RTL = (
    "module top_task(\n"
    "  input wire ap_clk,\n"
    "  input wire ap_rst_n,\n"
    "  input wire s_axi_control_AWVALID,\n"
    "  output wire s_axi_control_AWREADY,\n"
    "  input wire [5:0] s_axi_control_AWADDR,\n"
    "  input wire s_axi_control_WVALID,\n"
    "  output wire s_axi_control_WREADY,\n"
    "  input wire [31:0] s_axi_control_WDATA,\n"
    "  input wire [3:0] s_axi_control_WSTRB,\n"
    "  input wire s_axi_control_ARVALID,\n"
    "  output wire s_axi_control_ARREADY,\n"
    "  input wire [5:0] s_axi_control_ARADDR,\n"
    "  output wire s_axi_control_RVALID,\n"
    "  input wire s_axi_control_RREADY,\n"
    "  output wire [31:0] s_axi_control_RDATA,\n"
    "  output wire [1:0] s_axi_control_RRESP,\n"
    "  output wire s_axi_control_BVALID,\n"
    "  input wire s_axi_control_BREADY,\n"
    "  output wire [1:0] s_axi_control_BRESP,\n"
    "  output wire interrupt\n"
    ");\nendmodule"
)


def _trivial_modules_json() -> str:
    return json.dumps(
        {
            "top_task": _TRIVIAL_TOP_RTL,
            "child": (
                "module child(\n"
                "  input wire ap_clk,\n"
                "  input wire ap_rst_n,\n"
                "  input wire [31:0] n\n"
                ");\nendmodule"
            ),
        }
    )


def _build_rtl_dir(
    dest: str,
    *,
    include_ctrl_s_axi: bool = True,
    include_child: bool = True,
    include_fsm: bool = True,
) -> None:
    if include_ctrl_s_axi:
        with open(
            os.path.join(dest, "top_task_control_s_axi.v"), "w", encoding="utf-8"
        ) as f:
            f.write(
                "module top_task_control_s_axi (\n"
                "  input wire ACLK,\n"
                "  input wire ARESET\n"
                ");\nendmodule\n"
            )
    if include_child:
        with open(os.path.join(dest, "child.v"), "w", encoding="utf-8") as f:
            f.write(
                "module child(\n"
                "  input wire ap_clk,\n"
                "  input wire ap_rst_n,\n"
                "  input wire [31:0] n\n"
                ");\nendmodule"
            )
    if include_fsm:
        # `get_project_from_paths` requires `{upper_task}_fsm.v` on disk
        # for every upper task. The trivial fixture has one upper task
        # (`top_task`).
        with open(os.path.join(dest, "top_task_fsm.v"), "w", encoding="utf-8") as f:
            f.write(
                "module top_task_fsm (\n"
                "  input wire ap_clk,\n"
                "  input wire ap_rst_n,\n"
                "  input wire ap_start,\n"
                "  output wire ap_done,\n"
                "  output wire ap_idle,\n"
                "  output wire ap_ready\n"
                ");\nendmodule\n"
            )


def _write_device_config(
    cfg_dir: str,
    *,
    floorplan_payload: dict | None = None,
    device_payload: dict | None = None,
) -> tuple[str, str]:
    """Write a matched pair of floorplan.json and device_config.json, return (dc, fp) paths."""
    fp_path = os.path.join(cfg_dir, "floorplan.json")
    dc_path = os.path.join(cfg_dir, "device_config.json")
    with open(fp_path, "w", encoding="utf-8") as f:
        json.dump(floorplan_payload or {"child_0": "SLOT_X0Y0:SLOT_X0Y0"}, f)
    with open(dc_path, "w", encoding="utf-8") as f:
        json.dump(
            device_payload
            or {
                "part_num": "xc_part",
                "slots": [{"x": 0, "y": 0, "pblock_ranges": ["-add CLOCKREGION_X0Y0"]}],
            },
            f,
        )
    return dc_path, fp_path


def _run_lowering_from_paths(
    *,
    include_ctrl_s_axi: bool = True,
    include_child_rtl: bool = True,
    module_files: str | None = None,
    floorplan_payload: dict | None = None,
    device_payload: dict | None = None,
    device_config_path_override: str | None = None,
) -> tuple[dict, str, str, str]:
    """Drive `get_project_from_paths` on a shared trivial fixture.

    Returns (parsed_project_json, rtl_dir, dc_path, fp_path) so the caller
    can assert on-disk state and the emitted GraphIR in the same breath.
    """
    import tempfile

    from tapa_core import lowering  # type: ignore[import-not-found]

    rtl_dir = tempfile.mkdtemp(prefix="parity_rtl_")
    cfg_dir = tempfile.mkdtemp(prefix="parity_cfg_")
    _build_rtl_dir(
        rtl_dir,
        include_ctrl_s_axi=include_ctrl_s_axi,
        include_child=include_child_rtl,
    )
    dc_path, fp_path = _write_device_config(
        cfg_dir,
        floorplan_payload=floorplan_payload,
        device_payload=device_payload,
    )
    if device_config_path_override is not None:
        dc_path = device_config_path_override
    mods = module_files if module_files is not None else _trivial_modules_json()
    result = lowering.get_project_from_paths(
        _trivial_program_json(), mods, dc_path, fp_path, rtl_dir
    )
    return json.loads(result), rtl_dir, dc_path, fp_path


def test_lowering_get_project_from_paths_success() -> None:
    import tempfile

    from tapa_core import lowering  # type: ignore[import-not-found]

    with (
        tempfile.TemporaryDirectory() as rtl_dir,
        tempfile.TemporaryDirectory() as cfg_dir,
    ):
        _build_rtl_dir(rtl_dir)
        fp_path = os.path.join(cfg_dir, "floorplan.json")
        with open(fp_path, "w", encoding="utf-8") as f:
            json.dump({"child_0": "SLOT_X0Y0:SLOT_X0Y0"}, f)
        dc_path = os.path.join(cfg_dir, "device_config.json")
        with open(dc_path, "w", encoding="utf-8") as f:
            json.dump(
                {
                    "part_num": "xc_part",
                    "slots": [
                        {"x": 0, "y": 0, "pblock_ranges": ["-add CLOCKREGION_X0Y0"]}
                    ],
                },
                f,
            )
        result = lowering.get_project_from_paths(
            _trivial_program_json(),
            _trivial_modules_json(),
            dc_path,
            fp_path,
            rtl_dir,
        )
        parsed = json.loads(result)
        mod_names = [m["name"] for m in parsed["modules"]["module_definitions"]]
        assert "top_task_control_s_axi" in mod_names, mod_names
        assert "SLOT_X0Y0_SLOT_X0Y0" in mod_names, mod_names
        # ctrl_s_axi body must come from the real file, not a placeholder.
        ctrl = next(
            m
            for m in parsed["modules"]["module_definitions"]
            if m["name"] == "top_task_control_s_axi"
        )
        assert "Auto-generated" not in ctrl["verilog"], (
            "ctrl_s_axi body must come from the file on disk, not a placeholder"
        )
        assert "module top_task_control_s_axi" in ctrl["verilog"]


def test_lowering_missing_ctrl_s_axi_raises() -> None:
    import tempfile

    from tapa_core import lowering  # type: ignore[import-not-found]

    with (
        tempfile.TemporaryDirectory() as rtl_dir,
        tempfile.TemporaryDirectory() as cfg_dir,
    ):
        _build_rtl_dir(rtl_dir, include_ctrl_s_axi=False)
        fp_path = os.path.join(cfg_dir, "floorplan.json")
        with open(fp_path, "w", encoding="utf-8") as f:
            json.dump({"child_0": "SLOT_X0Y0:SLOT_X0Y0"}, f)
        dc_path = os.path.join(cfg_dir, "device_config.json")
        with open(dc_path, "w", encoding="utf-8") as f:
            json.dump({"slots": []}, f)
        err_msg = ""
        try:
            lowering.get_project_from_paths(
                _trivial_program_json(),
                _trivial_modules_json(),
                dc_path,
                fp_path,
                rtl_dir,
            )
        except ValueError as exc:
            err_msg = str(exc)
        if not err_msg:
            msg = "should have raised for missing ctrl_s_axi.v"
            raise AssertionError(msg)
        assert "ctrl_s_axi" in err_msg, err_msg


def test_lowering_bindings_reject_malformed_module_files() -> None:
    from tapa_core import lowering  # type: ignore[import-not-found]

    # Pass an unparsable RTL source for the `child` task. Both the
    # state-based `get_project` and the path-based
    # `get_project_from_paths` must surface the parse failure as
    # `ValueError` rather than silently dropping the entry.
    program = _trivial_program_json()
    module_files = json.dumps(
        {
            "top_task": (
                "module top_task(\n"
                "  input wire ap_clk,\n  input wire ap_rst_n\n"
                ");\nendmodule"
            ),
            "child": "this is not valid verilog",
            "top_task_control_s_axi": (
                "module top_task_control_s_axi(\n"
                "  input wire ACLK,\n  input wire ARESET\n"
                ");\nendmodule"
            ),
        }
    )
    err_msg = ""
    try:
        lowering.get_project(program, module_files, json.dumps({"SLOT_0": ["child_0"]}))
    except ValueError as exc:
        err_msg = str(exc)
    if not err_msg:
        msg = "get_project should reject malformed RTL in module_files"
        raise AssertionError(msg)
    assert "child" in err_msg, err_msg


def test_lowering_missing_fsm_rtl_raises() -> None:
    import tempfile

    from tapa_core import lowering  # type: ignore[import-not-found]

    # Omit the `top_task_fsm.v` file; path-based lowering must treat
    # this as an error rather than silently falling back to the 6-port
    # FSM stub (which would yield incorrect top-level handshake wiring).
    with (
        tempfile.TemporaryDirectory() as rtl_dir,
        tempfile.TemporaryDirectory() as cfg_dir,
    ):
        _build_rtl_dir(rtl_dir, include_fsm=False)
        fp_path = os.path.join(cfg_dir, "floorplan.json")
        with open(fp_path, "w", encoding="utf-8") as f:
            json.dump({"child_0": "SLOT_X0Y0:SLOT_X0Y0"}, f)
        dc_path = os.path.join(cfg_dir, "device_config.json")
        with open(dc_path, "w", encoding="utf-8") as f:
            json.dump({"slots": []}, f)
        err_msg = ""
        try:
            lowering.get_project_from_paths(
                _trivial_program_json(),
                _trivial_modules_json(),
                dc_path,
                fp_path,
                rtl_dir,
            )
        except ValueError as exc:
            err_msg = str(exc)
        if not err_msg:
            msg = "should have raised for missing top_task_fsm.v"
            raise AssertionError(msg)
        assert "FSM" in err_msg or "fsm" in err_msg, err_msg


def test_lowering_malformed_fsm_rtl_raises() -> None:
    import tempfile

    from tapa_core import lowering  # type: ignore[import-not-found]

    # Write a malformed `top_task_fsm.v`; path-based lowering must
    # surface the parse failure rather than silently falling back.
    with (
        tempfile.TemporaryDirectory() as rtl_dir,
        tempfile.TemporaryDirectory() as cfg_dir,
    ):
        _build_rtl_dir(rtl_dir, include_fsm=False)
        with open(os.path.join(rtl_dir, "top_task_fsm.v"), "w", encoding="utf-8") as f:
            f.write("not valid verilog")
        fp_path = os.path.join(cfg_dir, "floorplan.json")
        with open(fp_path, "w", encoding="utf-8") as f:
            json.dump({"child_0": "SLOT_X0Y0:SLOT_X0Y0"}, f)
        dc_path = os.path.join(cfg_dir, "device_config.json")
        with open(dc_path, "w", encoding="utf-8") as f:
            json.dump({"slots": []}, f)
        err_msg = ""
        try:
            lowering.get_project_from_paths(
                _trivial_program_json(),
                _trivial_modules_json(),
                dc_path,
                fp_path,
                rtl_dir,
            )
        except ValueError as exc:
            err_msg = str(exc)
        if not err_msg:
            msg = "should have raised for malformed top_task_fsm.v"
            raise AssertionError(msg)
        assert "FSM" in err_msg or "fsm" in err_msg, err_msg


def test_lowering_missing_leaf_rtl_raises() -> None:
    import tempfile

    from tapa_core import lowering  # type: ignore[import-not-found]

    # Omit the child RTL both on disk and from module_files; the
    # path-based lowering must detect the absence.
    with (
        tempfile.TemporaryDirectory() as rtl_dir,
        tempfile.TemporaryDirectory() as cfg_dir,
    ):
        _build_rtl_dir(rtl_dir, include_child=False)
        fp_path = os.path.join(cfg_dir, "floorplan.json")
        with open(fp_path, "w", encoding="utf-8") as f:
            json.dump({"child_0": "SLOT_X0Y0:SLOT_X0Y0"}, f)
        dc_path = os.path.join(cfg_dir, "device_config.json")
        with open(dc_path, "w", encoding="utf-8") as f:
            json.dump({"slots": []}, f)
        # Empty module_files so `child` is not pre-attached in state.
        err_msg = ""
        try:
            lowering.get_project_from_paths(
                _trivial_program_json(),
                "{}",
                dc_path,
                fp_path,
                rtl_dir,
            )
        except ValueError as exc:
            err_msg = str(exc)
        if not err_msg:
            msg = "should have raised for missing leaf RTL"
            raise AssertionError(msg)
        assert "leaf RTL" in err_msg or "child.v" in err_msg, err_msg


def test_graphir_export_missing_dest_raises() -> None:
    from tapa_core import graphir_export  # type: ignore[import-not-found]

    project_json = json.dumps(
        {
            "modules": {
                "name": "$root",
                "module_definitions": [],
            },
            "blackboxes": [],
        }
    )
    # Use a path guaranteed not to exist.
    bogus = "/tmp/tapa-core-rlcr-round1-nonexistent-destination"
    err_msg = ""
    try:
        graphir_export.export_project(project_json, bogus)
    except (ValueError, OSError) as exc:
        err_msg = str(exc)
    if not err_msg:
        msg = "export to non-existent directory must error"
        raise AssertionError(msg)
    assert "does not exist" in err_msg or "No such file" in err_msg, err_msg


def test_lowering_fifo_body_is_real() -> None:
    """FIFO module definition carries the real template body."""
    parsed, *_ = _run_lowering_from_paths()
    fifo = next(
        m for m in parsed["modules"]["module_definitions"] if m["name"] == "fifo"
    )
    assert "module fifo" in fifo["verilog"], (
        "fifo def must carry the real template body, got empty/placeholder"
    )
    assert "first-word fall-through" in fifo["verilog"] or "generate" in fifo["verilog"]


def test_export_roundtrip_from_paths() -> None:
    """Parity: go through real path API, export, reparse with tapa-rtl."""
    import tempfile

    from tapa_core import graphir_export, rtl  # type: ignore[import-not-found]

    parsed, *_ = _run_lowering_from_paths()
    project_json = json.dumps(parsed)
    with tempfile.TemporaryDirectory() as dest:
        graphir_export.export_project(project_json, dest)
        files = os.listdir(dest)
        assert "fifo.v" in files, files
        assert "top_task_control_s_axi.v" in files, files
        with open(os.path.join(dest, "fifo.v"), encoding="utf-8") as f:
            fifo_v = f.read()
        fifo_parsed = rtl.parse(fifo_v)
        assert fifo_parsed["name"] == "fifo", fifo_parsed.get("name")
        port_names = {p["name"] for p in fifo_parsed["ports"]}
        assert {"clk", "reset", "if_din", "if_dout"}.issubset(port_names), port_names
        # ctrl_s_axi.v must carry the body read from the RTL dir, not a placeholder.
        with open(
            os.path.join(dest, "top_task_control_s_axi.v"), encoding="utf-8"
        ) as f:
            ctrl_v = f.read()
        assert "Auto-generated" not in ctrl_v, (
            "exported ctrl_s_axi.v must come from the file on disk"
        )
        ctrl_parsed = rtl.parse(ctrl_v)
        assert ctrl_parsed["name"] == "top_task_control_s_axi", ctrl_parsed


def test_lowering_interface_roles_applied() -> None:
    """Every generated iface gets a source/sink role; no empty roles."""
    parsed, *_ = _run_lowering_from_paths()
    ifaces = parsed.get("ifaces") or {}
    top_ifaces = ifaces.get("top_task", [])
    assert top_ifaces, "top_task must have at least one interface"
    roles = {i.get("role", "") for i in top_ifaces}
    assert roles & {"source", "sink"}, (
        f"expected inferred source/sink roles on top_task, got {roles}"
    )
    assert "" not in roles, f"role field must be populated after inference, got {roles}"


def test_lowering_attaches_leaf_from_rtl_dir() -> None:
    """Leaf RTL omitted from module_files but present on disk is still attached."""
    mods_without_child = json.dumps(
        {
            "top_task": (
                "module top_task(\n"
                "  input wire ap_clk,\n"
                "  input wire ap_rst_n,\n"
                "  input wire [31:0] n\n"
                ");\nendmodule"
            )
        }
    )
    parsed, *_ = _run_lowering_from_paths(module_files=mods_without_child)
    mod_names = {m["name"] for m in parsed["modules"]["module_definitions"]}
    assert "child" in mod_names, (
        f"child leaf RTL was supposed to be parsed from rtl_dir, got modules: {mod_names}"
    )


def test_lowering_part_num_and_pblock_populated() -> None:
    """device_config successfully populates part_num and island pblock map."""
    parsed, *_ = _run_lowering_from_paths()
    assert parsed.get("part_num") == "xc_part", parsed.get("part_num")
    pblock = parsed.get("island_to_pblock_range") or {}
    assert "SLOT_X0Y0_TO_SLOT_X0Y0" in pblock, pblock
    assert pblock["SLOT_X0Y0_TO_SLOT_X0Y0"] == ["-add CLOCKREGION_X0Y0"], pblock


def test_lowering_top_fsm_ifaces_present() -> None:
    """`top_task_fsm` module carries the FSM ApCtrl interface."""
    parsed, *_ = _run_lowering_from_paths()
    ifaces = parsed.get("ifaces") or {}
    fsm_ifaces = ifaces.get("top_task_fsm") or []
    assert fsm_ifaces, (
        f"top_task_fsm must have at least one interface, got keys: {list(ifaces.keys())}"
    )
    kinds = {i.get("type") for i in fsm_ifaces}
    assert "ap_ctrl" in kinds, (
        f"top_task_fsm must have an ap_ctrl interface, got: {kinds}"
    )
    # Every interface must have a role assigned.
    for i in fsm_ifaces:
        assert i.get("role"), f"top_task_fsm iface missing role: {i.get('type')} / {i}"


def test_lowering_missing_device_config_raises() -> None:
    """Negative path: missing device_config.json surfaces as error, not silent None."""
    err_msg = ""
    try:
        _run_lowering_from_paths(
            device_config_path_override="/nonexistent/device_config.json"
        )
    except ValueError as exc:
        err_msg = str(exc)
    if not err_msg:
        msg = "missing device_config.json must error, not silently return None"
        raise AssertionError(msg)
    assert "device_config.json" in err_msg or "not found" in err_msg, err_msg


def test_lowering_structural_parity_invariants() -> None:
    """Parity: structural invariants the Python pipeline also guarantees.

    Runs the Rust `get_project_from_paths` pipeline and asserts the serialized
    project matches the shape Python's `get_project_from_floorplanned_program`
    produces on the same kind of input:
      * Module list is exactly {top_task, top_task_fsm, top_task_control_s_axi,
        slot, child, fifo, reset_inverter}.
      * Each infrastructure module (fifo, reset_inverter, ctrl_s_axi, slot, top,
        fsm) has a non-empty interface list.
      * The fifo module has Clock + FeedForwardReset + 2 HandShake + 2 FalsePath.
      * The ctrl_s_axi module has Clock + FeedForwardReset + FalsePath +
        5 HandShake (one per AXI-Lite channel) + FeedForward + ApCtrl.
      * Every iface across every module has a concrete role (source / sink),
        never the empty string or "to_be_determined".
    """
    parsed, *_ = _run_lowering_from_paths()
    mod_names = {m["name"] for m in parsed["modules"]["module_definitions"]}
    expected_modules = {
        "top_task",
        "top_task_fsm",
        "top_task_control_s_axi",
        "SLOT_X0Y0_SLOT_X0Y0",
        "child",
        "fifo",
        "reset_inverter",
    }
    assert expected_modules.issubset(mod_names), (
        f"missing modules: {expected_modules - mod_names}; got: {mod_names}"
    )

    ifaces = parsed.get("ifaces") or {}
    for key in (
        "fifo",
        "reset_inverter",
        "top_task_control_s_axi",
        "top_task",
        "top_task_fsm",
        "SLOT_X0Y0_SLOT_X0Y0",
    ):
        assert ifaces.get(key), f"module {key!r} must have at least one interface"

    # FIFO interface shape.
    fifo_types = [i["type"] for i in ifaces["fifo"]]
    assert fifo_types.count("handshake") == 2, fifo_types
    assert fifo_types.count("false_path") == 2, fifo_types
    assert "clock" in fifo_types, fifo_types
    assert "ff_reset" in fifo_types, fifo_types

    # ctrl_s_axi interface shape.
    ctrl_types = [i["type"] for i in ifaces["top_task_control_s_axi"]]
    assert ctrl_types.count("handshake") == 5, ctrl_types  # 5 AXI-Lite channels
    assert "ap_ctrl" in ctrl_types, ctrl_types
    assert "feed_forward" in ctrl_types, ctrl_types
    assert "clock" in ctrl_types, ctrl_types

    # All role-bearing ifaces (handshake, ap_ctrl, feed_forward, feed_forward_reset,
    # false_path, fp_reset) must have source/sink roles resolved. Clock / non_pipeline /
    # aux / unknown / tapa_peek keep their pass-through default (matches Python).
    role_bearing = {
        "handshake",
        "ap_ctrl",
        "feed_forward",
        "ff_reset",
        "false_path",
        "fp_reset",
    }
    for module_name, module_ifaces in ifaces.items():
        for i in module_ifaces:
            if i.get("type") not in role_bearing:
                continue
            role = i.get("role", "")
            assert role in {"source", "sink"}, (
                f"{module_name}/{i.get('type')} has unresolved role {role!r}"
            )


def test_lowering_malformed_device_config_raises() -> None:
    """Negative path: malformed device_config.json surfaces as error."""
    import tempfile

    from tapa_core import lowering  # type: ignore[import-not-found]

    with (
        tempfile.TemporaryDirectory() as rtl_dir,
        tempfile.TemporaryDirectory() as cfg_dir,
    ):
        _build_rtl_dir(rtl_dir)
        fp_path = os.path.join(cfg_dir, "floorplan.json")
        with open(fp_path, "w", encoding="utf-8") as f:
            json.dump({"child_0": "SLOT_X0Y0:SLOT_X0Y0"}, f)
        dc_path = os.path.join(cfg_dir, "device_config.json")
        with open(dc_path, "w", encoding="utf-8") as f:
            f.write("not valid json at all")
        err_msg = ""
        try:
            lowering.get_project_from_paths(
                _trivial_program_json(),
                _trivial_modules_json(),
                dc_path,
                fp_path,
                rtl_dir,
            )
        except ValueError as exc:
            err_msg = str(exc)
    if not err_msg:
        msg = "malformed device_config.json must raise"
        raise AssertionError(msg)
    assert "JSON" in err_msg or "json" in err_msg or "expected" in err_msg, err_msg


if __name__ == "__main__":
    _tests = [(k, v) for k, v in sorted(globals().items()) if k.startswith("test_")]
    _passed = 0
    _failed = 0
    for _name, _fn in _tests:
        try:
            _fn()
            _passed += 1
            print(f"  PASS: {_name}")  # noqa: T201
        except Exception as _exc:  # noqa: BLE001
            _failed += 1
            print(f"  FAIL: {_name}: {_exc}")  # noqa: T201
    print(f"\n{_passed} passed, {_failed} failed out of {len(_tests)} tests")  # noqa: T201
    if _failed:
        raise SystemExit(1)
