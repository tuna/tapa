//! PyO3 bindings for TAPA schema crates.
//!
//! Exposes `tapa_core.protocol`, `tapa_core.task_graph`, and
//! `tapa_core.graphir` submodules to Python.

use pyo3::prelude::*;

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
    use tapa_protocol::*;

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

        // ── Dict constants ──
        let port_widths = PyDict::new(py);
        for (k, v) in M_AXI_PORT_WIDTHS.iter() {
            port_widths.set_item(k, v)?;
        }
        m.add("M_AXI_PORT_WIDTHS", port_widths)?;

        let stream_dir = PyDict::new(py);
        for (k, v) in STREAM_PORT_DIRECTION.iter() {
            stream_dir.set_item(k, v)?;
        }
        m.add("STREAM_PORT_DIRECTION", stream_dir)?;

        let stream_opp = PyDict::new(py);
        for (k, v) in STREAM_PORT_OPPOSITE.iter() {
            stream_opp.set_item(k, v)?;
        }
        m.add("STREAM_PORT_OPPOSITE", stream_opp)?;

        let stream_w = PyDict::new(py);
        for (k, v) in STREAM_PORT_WIDTH.iter() {
            stream_w.set_item(k, v)?;
        }
        m.add("STREAM_PORT_WIDTH", stream_w)?;

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
        for (channel, entries) in M_AXI_PORTS.iter() {
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
        for (channel, info) in M_AXI_SUFFIXES_BY_CHANNEL.iter() {
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

/// Root Python module: `tapa_core`.
#[pymodule]
fn tapa_core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    protocol_mod::register(m)?;
    task_graph_mod::register(m)?;
    graphir_mod::register(m)?;
    Ok(())
}
