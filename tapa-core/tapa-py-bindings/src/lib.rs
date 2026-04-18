//! `PyO3` bindings for TAPA schema crates.
//!
//! Exposes `tapa_core.protocol`, `tapa_core.task_graph`,
//! `tapa_core.graphir`, `tapa_core.rtl`, and `tapa_core.topology`
//! submodules to Python.

use pyo3::prelude::*;

mod xilinx;

/// Convert a `serde_json::Value` to a Python object via Python's json module.
fn json_value_to_py(py: Python<'_>, val: &serde_json::Value) -> PyResult<PyObject> {
    let json_str =
        serde_json::to_string(val).map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    let json_mod = py.import("json")?;
    let result = json_mod.call_method1("loads", (json_str,))?;
    Ok(result.unbind())
}

/// Protocol constants submodule.
mod protocol_mod {
    use pyo3::prelude::*;
    use pyo3::types::{PyDict, PyList, PyTuple};
    use tapa_protocol::{
        CLK_SENS_LIST, FIFO_READ_PORTS, FIFO_WRITE_PORTS, HANDSHAKE_CLK, HANDSHAKE_DONE,
        HANDSHAKE_IDLE, HANDSHAKE_INPUT_PORTS, HANDSHAKE_OUTPUT_PORTS, HANDSHAKE_READY,
        HANDSHAKE_RST, HANDSHAKE_RST_N, HANDSHAKE_START, ISTREAM_SUFFIXES, M_AXI_ADDR_PORTS,
        M_AXI_PARAM_PREFIX, M_AXI_PARAM_SUFFIXES, M_AXI_PORT_WIDTHS, M_AXI_PORTS, M_AXI_PREFIX,
        M_AXI_SUFFIXES, M_AXI_SUFFIXES_BY_CHANNEL, M_AXI_SUFFIXES_COMPACT, OSTREAM_SUFFIXES,
        RTL_SUFFIX, SENS_TYPE, STREAM_DATA_SUFFIXES, STREAM_PORT_DIRECTION, STREAM_PORT_OPPOSITE,
        STREAM_PORT_WIDTH, S_AXI_NAME, PortDir,
    };

    pub fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
        let py = parent.py();
        let m = PyModule::new(py, "protocol")?;

        // ── Simple string constants ──
        m.add("HANDSHAKE_CLK", HANDSHAKE_CLK)?;
        m.add("HANDSHAKE_RST", HANDSHAKE_RST)?;
        m.add("HANDSHAKE_RST_N", HANDSHAKE_RST_N)?;
        m.add("HANDSHAKE_START", HANDSHAKE_START)?;
        m.add("HANDSHAKE_DONE", HANDSHAKE_DONE)?;
        m.add("HANDSHAKE_IDLE", HANDSHAKE_IDLE)?;
        m.add("HANDSHAKE_READY", HANDSHAKE_READY)?;
        m.add("SENS_TYPE", SENS_TYPE)?;
        m.add("CLK_SENS_LIST", CLK_SENS_LIST.as_str())?;
        m.add("RTL_SUFFIX", RTL_SUFFIX)?;
        m.add("S_AXI_NAME", S_AXI_NAME)?;
        m.add("M_AXI_PREFIX", M_AXI_PREFIX)?;
        m.add("M_AXI_PARAM_PREFIX", M_AXI_PARAM_PREFIX)?;

        // ── Tuple constants ──
        m.add("HANDSHAKE_INPUT_PORTS", PyTuple::new(py, HANDSHAKE_INPUT_PORTS)?)?;
        m.add("HANDSHAKE_OUTPUT_PORTS", PyTuple::new(py, HANDSHAKE_OUTPUT_PORTS)?)?;
        m.add("ISTREAM_SUFFIXES", PyTuple::new(py, ISTREAM_SUFFIXES)?)?;
        m.add("OSTREAM_SUFFIXES", PyTuple::new(py, OSTREAM_SUFFIXES)?)?;
        m.add("STREAM_DATA_SUFFIXES", PyTuple::new(py, STREAM_DATA_SUFFIXES)?)?;
        m.add("FIFO_READ_PORTS", PyTuple::new(py, FIFO_READ_PORTS)?)?;
        m.add("FIFO_WRITE_PORTS", PyTuple::new(py, FIFO_WRITE_PORTS)?)?;
        m.add("M_AXI_SUFFIXES_COMPACT", PyTuple::new(py, M_AXI_SUFFIXES_COMPACT)?)?;
        m.add("M_AXI_SUFFIXES", PyTuple::new(py, M_AXI_SUFFIXES.as_slice())?)?;
        m.add("M_AXI_PARAM_SUFFIXES", PyTuple::new(py, M_AXI_PARAM_SUFFIXES)?)?;

        // ── Dict constants (sorted keys for deterministic insertion order) ──
        macro_rules! sorted_dict {
            ($py:expr, $map:expr) => {{
                let dict = PyDict::new($py);
                let mut entries: Vec<_> = $map.iter().collect();
                entries.sort_by_key(|(k, _)| *k);
                for (k, v) in entries {
                    dict.set_item(k, v)?;
                }
                dict
            }};
        }

        m.add("M_AXI_PORT_WIDTHS", sorted_dict!(py, M_AXI_PORT_WIDTHS))?;
        m.add("STREAM_PORT_DIRECTION", sorted_dict!(py, STREAM_PORT_DIRECTION))?;
        m.add("STREAM_PORT_OPPOSITE", sorted_dict!(py, STREAM_PORT_OPPOSITE))?;
        m.add("STREAM_PORT_WIDTH", sorted_dict!(py, STREAM_PORT_WIDTH))?;

        // ── M_AXI_ADDR_PORTS: tuple of (name, direction) tuples ──
        let addr_ports = PyList::empty(py);
        for (name, dir) in M_AXI_ADDR_PORTS {
            let d = match dir {
                PortDir::Input => "input",
                PortDir::Output => "output",
            };
            addr_ports.append(PyTuple::new(py, [*name, d])?)?;
        }
        m.add("M_AXI_ADDR_PORTS", PyTuple::new(py, addr_ports)?)?;

        // ── M_AXI_PORTS: dict of channel -> tuple of (name, direction) ──
        let ports_dict = PyDict::new(py);
        let mut mp: Vec<_> = M_AXI_PORTS.iter().collect();
        mp.sort_by_key(|(k, _)| *k);
        for (channel, entries) in mp {
            let channel_list = PyList::empty(py);
            for (name, dir) in *entries {
                let d = match dir {
                    PortDir::Input => "input",
                    PortDir::Output => "output",
                };
                channel_list.append(PyTuple::new(py, [*name, d])?)?;
            }
            ports_dict.set_item(channel, PyTuple::new(py, channel_list)?)?;
        }
        m.add("M_AXI_PORTS", ports_dict)?;

        // ── M_AXI_SUFFIXES_BY_CHANNEL: dict of channel -> dict ──
        let by_channel = PyDict::new(py);
        let mut bc: Vec<_> = M_AXI_SUFFIXES_BY_CHANNEL.iter().collect();
        bc.sort_by_key(|(k, _)| *k);
        for (channel, info) in bc {
            let ch = PyDict::new(py);
            ch.set_item("ports", PyTuple::new(py, info.ports)?)?;
            ch.set_item("valid", info.valid)?;
            ch.set_item("ready", info.ready)?;
            by_channel.set_item(channel, ch)?;
        }
        m.add("M_AXI_SUFFIXES_BY_CHANNEL", by_channel)?;

        parent.add_submodule(&m)?;
        Ok(())
    }
}

/// Task graph submodule.
mod task_graph_mod {
    use pyo3::exceptions::PyValueError;
    use pyo3::prelude::*;

    #[pyfunction]
    fn parse(json_str: &str) -> PyResult<PyObject> {
        let graph = tapa_task_graph::Graph::from_json(json_str)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        let value =
            serde_json::to_value(&graph).map_err(|e| PyValueError::new_err(e.to_string()))?;
        Python::with_gil(|py| crate::json_value_to_py(py, &value))
    }

    #[pyfunction]
    fn validate(json_str: &str) -> PyResult<()> {
        tapa_task_graph::Graph::from_json(json_str)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(())
    }

    #[pyfunction]
    fn serialize(json_str: &str) -> PyResult<String> {
        let graph = tapa_task_graph::Graph::from_json(json_str)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        graph
            .to_json()
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    pub fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
        let m = PyModule::new(parent.py(), "task_graph")?;
        m.add_function(wrap_pyfunction!(parse, &m)?)?;
        m.add_function(wrap_pyfunction!(validate, &m)?)?;
        m.add_function(wrap_pyfunction!(serialize, &m)?)?;
        parent.add_submodule(&m)?;
        Ok(())
    }
}

/// `GraphIR` submodule.
mod graphir_mod {
    use pyo3::exceptions::PyValueError;
    use pyo3::prelude::*;

    #[pyfunction]
    fn parse(json_str: &str) -> PyResult<PyObject> {
        let project = tapa_graphir::Project::from_json(json_str)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        let value =
            serde_json::to_value(&project).map_err(|e| PyValueError::new_err(e.to_string()))?;
        Python::with_gil(|py| crate::json_value_to_py(py, &value))
    }

    #[pyfunction]
    fn validate(json_str: &str) -> PyResult<()> {
        tapa_graphir::Project::from_json(json_str)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(())
    }

    #[pyfunction]
    fn serialize(json_str: &str) -> PyResult<String> {
        let project = tapa_graphir::Project::from_json(json_str)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        project
            .to_json()
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    pub fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
        let m = PyModule::new(parent.py(), "graphir")?;
        m.add_function(wrap_pyfunction!(parse, &m)?)?;
        m.add_function(wrap_pyfunction!(validate, &m)?)?;
        m.add_function(wrap_pyfunction!(serialize, &m)?)?;
        parent.add_submodule(&m)?;
        Ok(())
    }
}

/// RTL interface parser submodule.
mod rtl_mod {
    use pyo3::exceptions::PyValueError;
    use pyo3::prelude::*;

    #[pyfunction]
    fn parse(verilog_str: &str) -> PyResult<PyObject> {
        let module = tapa_rtl::VerilogModule::parse(verilog_str)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        let value = serde_json::to_value(&module)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        Python::with_gil(|py| crate::json_value_to_py(py, &value))
    }

    #[pyfunction]
    fn classify_ports(verilog_str: &str) -> PyResult<PyObject> {
        let module = tapa_rtl::VerilogModule::parse(verilog_str)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        let classified = tapa_rtl::classify::classify_ports(&module.ports);
        // Return as dict keyed by port name (per planned API contract).
        let map: std::collections::HashMap<String, _> = classified.into_iter().collect();
        let value = serde_json::to_value(&map)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        Python::with_gil(|py| crate::json_value_to_py(py, &value))
    }

    /// Extract `(module_name, instance_name)` pairs for every submodule
    /// instantiation in a Verilog source string. Used by cross-language
    /// grouped-Verilog parity tests to compare instantiation shape
    /// structurally.
    #[pyfunction]
    fn extract_instance_names(verilog_str: &str) -> Vec<(String, String)> {
        tapa_rtl::parser::extract_instance_names(verilog_str)
    }

    pub fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
        let m = PyModule::new(parent.py(), "rtl")?;
        m.add_function(wrap_pyfunction!(parse, &m)?)?;
        m.add_function(wrap_pyfunction!(classify_ports, &m)?)?;
        m.add_function(wrap_pyfunction!(extract_instance_names, &m)?)?;
        parent.add_submodule(&m)?;
        Ok(())
    }
}

/// Topology / design.json submodule.
mod topology_mod {
    use pyo3::exceptions::PyValueError;
    use pyo3::prelude::*;

    #[pyfunction]
    fn parse_design(json_str: &str) -> PyResult<PyObject> {
        let design = tapa_topology::Design::from_json(json_str)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        let value = serde_json::to_value(&design.program)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        Python::with_gil(|py| crate::json_value_to_py(py, &value))
    }

    #[pyfunction]
    fn validate_design(json_str: &str) -> PyResult<()> {
        tapa_topology::Design::from_json(json_str)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(())
    }

    #[pyfunction]
    fn serialize_design(json_str: &str) -> PyResult<String> {
        let design = tapa_topology::Design::from_json(json_str)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        design
            .to_json()
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    pub fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
        let m = PyModule::new(parent.py(), "topology")?;
        m.add_function(wrap_pyfunction!(parse_design, &m)?)?;
        m.add_function(wrap_pyfunction!(validate_design, &m)?)?;
        m.add_function(wrap_pyfunction!(serialize_design, &m)?)?;
        parent.add_submodule(&m)?;
        Ok(())
    }
}

/// Slotting submodule (`gen_slot_cpp`, `replace_function`).
mod slotting_mod {
    use pyo3::exceptions::PyValueError;
    use pyo3::prelude::*;

    #[pyfunction]
    #[pyo3(signature = (slot_name, top_name, ports_json, top_cpp))]
    fn gen_slot_cpp(
        slot_name: &str,
        top_name: &str,
        ports_json: &str,
        top_cpp: &str,
    ) -> PyResult<String> {
        let ports: Vec<serde_json::Value> = serde_json::from_str(ports_json)
            .map_err(|e| PyValueError::new_err(format!("invalid ports JSON: {e}")))?;

        let slot_ports: Vec<tapa_slotting::SlotPort> = ports
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let cat = p["cat"]
                    .as_str()
                    .ok_or_else(|| PyValueError::new_err(format!("port[{i}] missing 'cat' field")))?
                    .to_owned();
                let name = p["name"]
                    .as_str()
                    .ok_or_else(|| PyValueError::new_err(format!("port[{i}] missing 'name' field")))?
                    .to_owned();
                let port_type = p["type"]
                    .as_str()
                    .ok_or_else(|| PyValueError::new_err(format!("port[{i}] missing 'type' field")))?
                    .to_owned();
                Ok(tapa_slotting::SlotPort {
                    cat,
                    name,
                    port_type,
                })
            })
            .collect::<PyResult<Vec<_>>>()?;

        tapa_slotting::gen_slot_cpp(slot_name, top_name, &slot_ports, top_cpp)
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    #[pyfunction]
    #[pyo3(signature = (source, func_name, new_body, new_def=None))]
    fn replace_function(
        source: &str,
        func_name: &str,
        new_body: &str,
        new_def: Option<&str>,
    ) -> PyResult<String> {
        tapa_slotting::cpp_surgery::replace_function(source, func_name, new_body, new_def)
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    #[pyfunction]
    fn get_floorplan_graph(graph_json: &str, slot_to_insts_json: &str) -> PyResult<String> {
        let graph: serde_json::Value = serde_json::from_str(graph_json)
            .map_err(|e| PyValueError::new_err(format!("invalid graph JSON: {e}")))?;
        let slot_to_insts: std::collections::BTreeMap<String, Vec<String>> =
            serde_json::from_str(slot_to_insts_json)
                .map_err(|e| PyValueError::new_err(format!("invalid slot_to_insts JSON: {e}")))?;
        let result = tapa_slotting::floorplan::get_floorplan_graph(&graph, &slot_to_insts)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        serde_json::to_string_pretty(&result)
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    pub fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
        let m = PyModule::new(parent.py(), "slotting")?;
        m.add_function(wrap_pyfunction!(gen_slot_cpp, &m)?)?;
        m.add_function(wrap_pyfunction!(replace_function, &m)?)?;
        m.add_function(wrap_pyfunction!(get_floorplan_graph, &m)?)?;
        parent.add_submodule(&m)?;
        Ok(())
    }
}

/// Codegen submodule (`attach_modules`, `generate_rtl`).
mod codegen_mod {
    use pyo3::exceptions::PyValueError;
    use pyo3::prelude::*;

    use std::collections::BTreeMap;

    /// Shared setup: parse design + modules, create state, attach all modules.
    fn setup_state(
        design_json: &str,
        module_files_json: &str,
    ) -> PyResult<tapa_codegen::rtl_state::TopologyWithRtl> {
        let program: tapa_topology::program::Program = serde_json::from_str(design_json)
            .map_err(|e| PyValueError::new_err(format!("invalid design JSON: {e}")))?;
        let module_files: BTreeMap<String, String> = serde_json::from_str(module_files_json)
            .map_err(|e| PyValueError::new_err(format!("invalid module_files JSON: {e}")))?;
        let mut state = tapa_codegen::rtl_state::TopologyWithRtl::new(program);
        for (task_name, verilog_source) in &module_files {
            let module = tapa_rtl::VerilogModule::parse(verilog_source)
                .map_err(|e| PyValueError::new_err(format!("parse error for {task_name}: {e}")))?;
            state
                .attach_module(task_name, module)
                .map_err(|e| PyValueError::new_err(e.to_string()))?;
        }
        Ok(state)
    }

    #[pyfunction]
    fn attach_modules(design_json: &str, module_files_json: &str) -> PyResult<PyObject> {
        let state = setup_state(design_json, module_files_json)?;

        let mut module_info = BTreeMap::new();
        for (name, mm) in &state.module_map {
            module_info.insert(name.clone(), serde_json::json!({
                "port_count": mm.inner.ports.len(),
                "signal_count": mm.inner.signals.len(),
            }));
        }

        let result = serde_json::json!({
            "attached_modules": module_info,
            "task_count": state.program.tasks.len(),
            "top": state.program.top,
        });
        Python::with_gil(|py| crate::json_value_to_py(py, &result))
    }

    #[pyfunction]
    fn generate_rtl(design_json: &str, module_files_json: &str) -> PyResult<PyObject> {
        let mut state = setup_state(design_json, module_files_json)?;

        tapa_codegen::generate_rtl(&mut state)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;

        // Collect modified modules (emitted Verilog text)
        let mut modified_modules = BTreeMap::new();
        for (name, mm) in &state.module_map {
            modified_modules.insert(name.clone(), mm.emit());
        }
        for (name, mm) in &state.fsm_modules {
            modified_modules.insert(format!("{name}_fsm"), mm.emit());
        }

        let result = serde_json::json!({
            "generated_files": state.generated_files,
            "modified_modules": modified_modules,
        });

        Python::with_gil(|py| crate::json_value_to_py(py, &result))
    }

    pub fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
        let m = PyModule::new(parent.py(), "codegen")?;
        m.add_function(wrap_pyfunction!(attach_modules, &m)?)?;
        m.add_function(wrap_pyfunction!(generate_rtl, &m)?)?;
        parent.add_submodule(&m)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// tapa_core.floorplan — ABGraph generation
// ---------------------------------------------------------------------------

mod floorplan_mod {
    use std::collections::BTreeMap;

    use pyo3::exceptions::PyValueError;
    use pyo3::prelude::*;

    #[pyfunction]
    fn get_top_level_ab_graph(
        program_json: &str,
        preassignments_json: &str,
        fsm_name: &str,
    ) -> PyResult<String> {
        let program: tapa_topology::program::Program =
            serde_json::from_str(program_json)
                .map_err(|e| PyValueError::new_err(format!("invalid program JSON: {e}")))?;
        let preassignments: BTreeMap<String, String> =
            serde_json::from_str(preassignments_json)
                .map_err(|e| PyValueError::new_err(format!("invalid preassignments JSON: {e}")))?;

        let graph = tapa_floorplan::gen_abgraph::get_top_level_ab_graph(
            &program,
            &preassignments,
            fsm_name,
        )
        .map_err(|e| PyValueError::new_err(e.to_string()))?;

        serde_json::to_string(&graph)
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    /// RTL-aware entrypoint: accepts module Verilog sources for FIFO width derivation.
    #[pyfunction]
    fn get_top_level_ab_graph_from_rtl(
        program_json: &str,
        module_files_json: &str,
        preassignments_json: &str,
        fsm_name: &str,
    ) -> PyResult<String> {
        let program: tapa_topology::program::Program =
            serde_json::from_str(program_json)
                .map_err(|e| PyValueError::new_err(format!("invalid program JSON: {e}")))?;
        let module_files: BTreeMap<String, String> =
            serde_json::from_str(module_files_json)
                .map_err(|e| PyValueError::new_err(format!("invalid modules JSON: {e}")))?;
        let preassignments: BTreeMap<String, String> =
            serde_json::from_str(preassignments_json)
                .map_err(|e| PyValueError::new_err(format!("invalid preassignments JSON: {e}")))?;

        // Parse and attach every entry in `module_files_json`. The
        // RTL-aware floorplan path derives FIFO widths from the parsed
        // producer RTL; silently skipping malformed Verilog or
        // mistyped task names would let `get_top_level_ab_graph_from_rtl`
        // fall back to topology-derived widths and return a
        // structurally-correct but semantically-stale AB graph. Surface
        // both parse and attach failures as `ValueError` so callers
        // can diagnose bad inputs rather than getting degraded output.
        let mut state = tapa_codegen::rtl_state::TopologyWithRtl::new(program);
        for (name, source) in &module_files {
            if !state.program.tasks.contains_key(name) {
                // Entry for an unknown task — reject rather than
                // silently ignore, since a mistyped key would
                // otherwise look like a successful attach.
                return Err(PyValueError::new_err(format!(
                    "module_files_json references unknown task `{name}`"
                )));
            }
            let module = tapa_rtl::VerilogModule::parse(source).map_err(|e| {
                PyValueError::new_err(format!(
                    "failed to parse RTL for `{name}`: {e}"
                ))
            })?;
            state.attach_module(name, module).map_err(|e| {
                PyValueError::new_err(format!(
                    "failed to attach RTL for `{name}`: {e}"
                ))
            })?;
        }

        let graph = tapa_floorplan::gen_abgraph::get_top_level_ab_graph_from_rtl(
            &state,
            &preassignments,
            fsm_name,
        )
        .map_err(|e| PyValueError::new_err(e.to_string()))?;

        serde_json::to_string(&graph)
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    pub fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
        let m = PyModule::new(parent.py(), "floorplan")?;
        m.add_function(wrap_pyfunction!(get_top_level_ab_graph, &m)?)?;
        m.add_function(wrap_pyfunction!(get_top_level_ab_graph_from_rtl, &m)?)?;
        parent.add_submodule(&m)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// tapa_core.lowering — GraphIR Project construction
// ---------------------------------------------------------------------------

mod lowering_mod {
    use std::collections::BTreeMap;

    use pyo3::exceptions::PyValueError;
    use pyo3::prelude::*;

    /// Build a `GraphIR` Project from a floorplanned program.
    ///
    /// Requires the caller to supply the real `{top}_control_s_axi`
    /// Verilog source in `module_files_json` (keyed by
    /// `{top}_control_s_axi`); the lowering pipeline refuses to
    /// fabricate a placeholder since placeholder bodies leaked through
    /// the exporter as `.v` files without a `module ... endmodule`
    /// declaration. Malformed RTL in `module_files_json` is surfaced
    /// as a `ValueError` rather than silently dropped.
    #[pyfunction]
    #[pyo3(signature = (program_json, module_files_json, slot_to_instances_json, pblock_ranges_json=None, part_num=None))]
    fn get_project(
        program_json: &str,
        module_files_json: &str,
        slot_to_instances_json: &str,
        pblock_ranges_json: Option<&str>,
        part_num: Option<String>,
    ) -> PyResult<String> {
        let program: tapa_topology::program::Program =
            serde_json::from_str(program_json)
                .map_err(|e| PyValueError::new_err(format!("invalid program JSON: {e}")))?;
        let module_files: BTreeMap<String, String> =
            serde_json::from_str(module_files_json)
                .map_err(|e| PyValueError::new_err(format!("invalid module_files JSON: {e}")))?;
        let slot_to_instances: BTreeMap<String, Vec<String>> =
            serde_json::from_str(slot_to_instances_json)
                .map_err(|e| PyValueError::new_err(format!("invalid slot mapping JSON: {e}")))?;

        // Extract the required `{top}_control_s_axi` source separately.
        let ctrl_s_axi_name = format!("{}_control_s_axi", program.top);
        let ctrl_s_axi_source =
            module_files.get(&ctrl_s_axi_name).cloned().ok_or_else(|| {
                PyValueError::new_err(format!(
                    "module_files_json missing required `{ctrl_s_axi_name}` RTL source"
                ))
            })?;

        // Build TopologyWithRtl, attach modules, and create FSM modules.
        // Malformed RTL or unknown task entries are surfaced as errors
        // so callers can diagnose bad inputs rather than silently
        // producing a project with degraded port/parameter data.
        let mut state = tapa_codegen::rtl_state::TopologyWithRtl::new(program);
        for (name, source) in &module_files {
            if *name == ctrl_s_axi_name {
                // Skip ctrl_s_axi: it's not a task and gets passed
                // through a separate channel to build_project_from_inputs.
                continue;
            }
            let module = tapa_rtl::VerilogModule::parse(source).map_err(|e| {
                PyValueError::new_err(format!(
                    "failed to parse RTL for `{name}`: {e}"
                ))
            })?;
            state.attach_module(name, module).map_err(|e| {
                PyValueError::new_err(format!(
                    "failed to attach RTL for `{name}`: {e}"
                ))
            })?;
        }
        // Create FSM modules for upper-level tasks that have attached modules
        let upper_task_names: Vec<_> = state.program.tasks.keys().cloned().collect();
        for task_name in upper_task_names {
            if state.is_upper_task(&task_name) && state.module_map.contains_key(&task_name) {
                let _ = state.create_fsm_module(&task_name);
            }
        }

        // Parse pblock ranges
        let pblock_ranges = pblock_ranges_json
            .map(|json| {
                serde_json::from_str::<BTreeMap<String, Vec<String>>>(json)
                    .map_err(|e| PyValueError::new_err(format!("invalid pblock JSON: {e}")))
            })
            .transpose()?;

        let project = tapa_lowering::build_project_from_inputs(
            &state,
            &ctrl_s_axi_source,
            &slot_to_instances,
            pblock_ranges,
            part_num,
        )
        .map_err(|e| PyValueError::new_err(e.to_string()))?;

        project
            .to_json()
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    /// Build a `GraphIR` Project from the path-based lowering boundary.
    ///
    /// Mirrors Python's `get_project_from_floorplanned_program`: reads
    /// `device_config.json`, `floorplan.json`, the leaf `.v` files, and the
    /// `{top}_control_s_axi.v` file from `rtl_dir`. Raises `ValueError` with
    /// a descriptive message on missing leaf RTL, missing `ctrl_s_axi`
    /// RTL, missing / malformed FSM RTL, or malformed entries in
    /// `module_files_json`.
    #[pyfunction]
    #[pyo3(signature = (program_json, module_files_json, device_config_path, floorplan_path, rtl_dir))]
    fn get_project_from_paths(
        program_json: &str,
        module_files_json: &str,
        device_config_path: &str,
        floorplan_path: &str,
        rtl_dir: &str,
    ) -> PyResult<String> {
        let program: tapa_topology::program::Program = serde_json::from_str(program_json)
            .map_err(|e| PyValueError::new_err(format!("invalid program JSON: {e}")))?;
        let module_files: BTreeMap<String, String> = serde_json::from_str(module_files_json)
            .map_err(|e| PyValueError::new_err(format!("invalid module_files JSON: {e}")))?;

        // Parse and attach every entry in `module_files_json`. Malformed
        // RTL or unknown task names are surfaced as `ValueError` so
        // callers can diagnose bad inputs rather than silently producing
        // a project with degraded port/parameter data. Entries keyed by
        // `{top}_control_s_axi` — infrastructure RTL that is not a
        // task — are skipped here; `build_project_from_paths` reads that
        // source from `rtl_dir` via `LoweringInputs`.
        let mut state = tapa_codegen::rtl_state::TopologyWithRtl::new(program);
        let ctrl_s_axi_name = format!("{}_control_s_axi", state.program.top);
        for (name, source) in &module_files {
            if *name == ctrl_s_axi_name {
                continue;
            }
            if !state.program.tasks.contains_key(name) {
                // Unknown task — ignore silently so callers can pass a
                // permissive set of extra modules without attaching
                // non-task artifacts.
                continue;
            }
            let module = tapa_rtl::VerilogModule::parse(source).map_err(|e| {
                PyValueError::new_err(format!(
                    "failed to parse RTL for `{name}`: {e}"
                ))
            })?;
            state.attach_module(name, module).map_err(|e| {
                PyValueError::new_err(format!(
                    "failed to attach RTL for `{name}`: {e}"
                ))
            })?;
        }
        let upper_task_names: Vec<_> = state.program.tasks.keys().cloned().collect();
        for task_name in upper_task_names {
            if state.is_upper_task(&task_name) && state.module_map.contains_key(&task_name) {
                let _ = state.create_fsm_module(&task_name);
            }
        }

        let inputs = tapa_lowering::LoweringInputs::new(
            &mut state,
            device_config_path,
            floorplan_path,
            rtl_dir,
        )
        .map_err(|e| PyValueError::new_err(e.to_string()))?;

        let project = tapa_lowering::build_project_from_paths(inputs)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;

        project
            .to_json()
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    pub fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
        let m = PyModule::new(parent.py(), "lowering")?;
        m.add_function(wrap_pyfunction!(get_project, &m)?)?;
        m.add_function(wrap_pyfunction!(get_project_from_paths, &m)?)?;
        parent.add_submodule(&m)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// tapa_core.graphir_export — Verilog export
// ---------------------------------------------------------------------------

mod graphir_export_mod {
    use std::path::Path;

    use pyo3::exceptions::PyValueError;
    use pyo3::prelude::*;

    #[pyfunction]
    fn export_project(project_json: &str, dest_path: &str) -> PyResult<()> {
        let project = tapa_graphir::Project::from_json(project_json)
            .map_err(|e| PyValueError::new_err(format!("invalid project JSON: {e}")))?;

        tapa_graphir_export::export_project(&project, Path::new(dest_path))
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    pub fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
        let m = PyModule::new(parent.py(), "graphir_export")?;
        m.add_function(wrap_pyfunction!(export_project, &m)?)?;
        parent.add_submodule(&m)?;
        Ok(())
    }
}

/// Register a submodule in `sys.modules` so dotted imports work.
fn register_submodule(parent: &Bound<'_, PyModule>, name: &str) -> PyResult<()> {
    let py = parent.py();
    let sys = py.import("sys")?;
    let modules = sys.getattr("modules")?;
    let parent_name = parent.name()?;
    let full_name = format!("{parent_name}.{name}");
    let submod = parent.getattr(name)?;
    modules.set_item(full_name, submod)?;
    Ok(())
}

/// Root Python module: `tapa_core`.
#[pymodule]
fn tapa_core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    protocol_mod::register(m)?;
    task_graph_mod::register(m)?;
    graphir_mod::register(m)?;
    rtl_mod::register(m)?;
    topology_mod::register(m)?;
    slotting_mod::register(m)?;
    codegen_mod::register(m)?;
    floorplan_mod::register(m)?;
    lowering_mod::register(m)?;
    graphir_export_mod::register(m)?;
    xilinx::register(m)?;

    // Register submodules in sys.modules for dotted imports.
    for name in [
        "protocol",
        "task_graph",
        "graphir",
        "rtl",
        "topology",
        "slotting",
        "codegen",
        "floorplan",
        "lowering",
        "graphir_export",
        "xilinx",
    ] {
        register_submodule(m, name)?;
    }
    Ok(())
}
