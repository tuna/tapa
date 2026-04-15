//! PyO3 bindings for TAPA schema crates.
//!
//! Exposes `tapa_core.protocol`, `tapa_core.task_graph`, and
//! `tapa_core.graphir` submodules to Python.

use pyo3::prelude::*;

/// Protocol constants submodule.
mod protocol_mod {
    use pyo3::prelude::*;
    use pyo3::types::PyDict;
    use tapa_protocol::*;

    #[pyfunction]
    fn handshake_clk() -> &'static str { HANDSHAKE_CLK }

    #[pyfunction]
    fn handshake_rst() -> &'static str { HANDSHAKE_RST }

    #[pyfunction]
    fn handshake_rst_n() -> &'static str { HANDSHAKE_RST_N }

    #[pyfunction]
    fn handshake_start() -> &'static str { HANDSHAKE_START }

    #[pyfunction]
    fn handshake_done() -> &'static str { HANDSHAKE_DONE }

    #[pyfunction]
    fn handshake_idle() -> &'static str { HANDSHAKE_IDLE }

    #[pyfunction]
    fn handshake_ready() -> &'static str { HANDSHAKE_READY }

    #[pyfunction]
    fn handshake_input_ports() -> Vec<&'static str> { HANDSHAKE_INPUT_PORTS.to_vec() }

    #[pyfunction]
    fn handshake_output_ports() -> Vec<&'static str> { HANDSHAKE_OUTPUT_PORTS.to_vec() }

    #[pyfunction]
    fn sens_type() -> &'static str { SENS_TYPE }

    #[pyfunction]
    fn clk_sens_list() -> String { CLK_SENS_LIST.clone() }

    #[pyfunction]
    fn rtl_suffix() -> &'static str { RTL_SUFFIX }

    #[pyfunction]
    fn istream_suffixes() -> Vec<&'static str> { ISTREAM_SUFFIXES.to_vec() }

    #[pyfunction]
    fn ostream_suffixes() -> Vec<&'static str> { OSTREAM_SUFFIXES.to_vec() }

    #[pyfunction]
    fn stream_data_suffixes() -> Vec<&'static str> { STREAM_DATA_SUFFIXES.to_vec() }

    #[pyfunction]
    fn fifo_read_ports() -> Vec<&'static str> { FIFO_READ_PORTS.to_vec() }

    #[pyfunction]
    fn fifo_write_ports() -> Vec<&'static str> { FIFO_WRITE_PORTS.to_vec() }

    #[pyfunction]
    fn s_axi_name() -> &'static str { S_AXI_NAME }

    #[pyfunction]
    fn m_axi_prefix() -> &'static str { M_AXI_PREFIX }

    #[pyfunction]
    fn m_axi_param_prefix() -> &'static str { M_AXI_PARAM_PREFIX }

    #[pyfunction]
    fn m_axi_suffixes_compact() -> Vec<&'static str> { M_AXI_SUFFIXES_COMPACT.to_vec() }

    #[pyfunction]
    fn m_axi_suffixes() -> Vec<&'static str> { M_AXI_SUFFIXES.to_vec() }

    #[pyfunction]
    fn m_axi_param_suffixes() -> Vec<&'static str> { M_AXI_PARAM_SUFFIXES.to_vec() }

    pub fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
        let m = PyModule::new(parent.py(), "protocol")?;
        m.add_function(wrap_pyfunction!(handshake_clk, &m)?)?;
        m.add_function(wrap_pyfunction!(handshake_rst, &m)?)?;
        m.add_function(wrap_pyfunction!(handshake_rst_n, &m)?)?;
        m.add_function(wrap_pyfunction!(handshake_start, &m)?)?;
        m.add_function(wrap_pyfunction!(handshake_done, &m)?)?;
        m.add_function(wrap_pyfunction!(handshake_idle, &m)?)?;
        m.add_function(wrap_pyfunction!(handshake_ready, &m)?)?;
        m.add_function(wrap_pyfunction!(handshake_input_ports, &m)?)?;
        m.add_function(wrap_pyfunction!(handshake_output_ports, &m)?)?;
        m.add_function(wrap_pyfunction!(sens_type, &m)?)?;
        m.add_function(wrap_pyfunction!(clk_sens_list, &m)?)?;
        m.add_function(wrap_pyfunction!(rtl_suffix, &m)?)?;
        m.add_function(wrap_pyfunction!(istream_suffixes, &m)?)?;
        m.add_function(wrap_pyfunction!(ostream_suffixes, &m)?)?;
        m.add_function(wrap_pyfunction!(stream_data_suffixes, &m)?)?;
        m.add_function(wrap_pyfunction!(fifo_read_ports, &m)?)?;
        m.add_function(wrap_pyfunction!(fifo_write_ports, &m)?)?;
        m.add_function(wrap_pyfunction!(s_axi_name, &m)?)?;
        m.add_function(wrap_pyfunction!(m_axi_prefix, &m)?)?;
        m.add_function(wrap_pyfunction!(m_axi_param_prefix, &m)?)?;
        m.add_function(wrap_pyfunction!(m_axi_suffixes_compact, &m)?)?;
        m.add_function(wrap_pyfunction!(m_axi_suffixes, &m)?)?;
        m.add_function(wrap_pyfunction!(m_axi_param_suffixes, &m)?)?;

        // Dict-valued constants
        let py = parent.py();
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

        // Simple string constants as module attributes
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

        parent.add_submodule(&m)?;
        Ok(())
    }
}

/// Task graph submodule.
mod task_graph_mod {
    use pyo3::prelude::*;
    use pyo3::exceptions::PyValueError;

    #[pyfunction]
    fn parse(json_str: &str) -> PyResult<PyObject> {
        let graph = tapa_task_graph::Graph::from_json(json_str)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        let value = serde_json::to_value(&graph)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        Python::with_gil(|py| json_value_to_py(py, &value))
    }

    #[pyfunction]
    fn validate(json_str: &str) -> PyResult<()> {
        tapa_task_graph::Graph::from_json(json_str)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(())
    }

    /// Convert a serde_json::Value to a Python object via Python's json module.
    pub fn json_value_to_py(py: Python<'_>, val: &serde_json::Value) -> PyResult<PyObject> {
        let json_str = serde_json::to_string(val)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        let json_mod = py.import("json")?;
        let result = json_mod.call_method1("loads", (json_str,))?;
        Ok(result.unbind())
    }

    pub fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
        let m = PyModule::new(parent.py(), "task_graph")?;
        m.add_function(wrap_pyfunction!(parse, &m)?)?;
        m.add_function(wrap_pyfunction!(validate, &m)?)?;
        parent.add_submodule(&m)?;
        Ok(())
    }
}

/// GraphIR submodule.
mod graphir_mod {
    use pyo3::prelude::*;
    use pyo3::exceptions::PyValueError;

    #[pyfunction]
    fn parse(json_str: &str) -> PyResult<PyObject> {
        let project = tapa_graphir::Project::from_json(json_str)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        let value = serde_json::to_value(&project)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        Python::with_gil(|py| crate::task_graph_mod::json_value_to_py(py, &value))
    }

    #[pyfunction]
    fn validate(json_str: &str) -> PyResult<()> {
        tapa_graphir::Project::from_json(json_str)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(())
    }

    pub fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
        let m = PyModule::new(parent.py(), "graphir")?;
        m.add_function(wrap_pyfunction!(parse, &m)?)?;
        m.add_function(wrap_pyfunction!(validate, &m)?)?;
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
