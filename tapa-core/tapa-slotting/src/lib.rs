//! Pre-HLS slot construction: slot grouping, C++ surgery, and port rewriting.
//!
//! This crate handles pre-HLS slot grouping from task graphs, FIFO/port
//! rewriting for slot boundaries, and C++ function generation via tree-sitter.

pub mod cpp_surgery;
pub mod error;
pub mod floorplan;
pub mod pragma;
pub mod slot;

use error::SlottingError;
use pragma::process_port;

/// A port specification for slot C++ generation.
pub struct SlotPort {
    /// Port category: istream, ostream, scalar, mmap, `async_mmap`, hmap, istreams, ostreams.
    pub cat: String,
    /// Port name (may include array index like `port[0]`).
    pub name: String,
    /// C++ type string.
    pub port_type: String,
}

/// Generate a slot C++ source by replacing the top-level function with a
/// slot function containing HLS pragmas for each port.
///
/// Uses 4-argument `replace_function` to remove old `extern "C"` blocks
/// for `top_name` and append new declaration + definition blocks for
/// `slot_name`.
pub fn gen_slot_cpp(
    slot_name: &str,
    top_name: &str,
    ports: &[SlotPort],
    top_cpp: &str,
) -> Result<String, SlottingError> {
    let mut cpp_ports = Vec::new();
    let mut cpp_pragmas = Vec::new();

    for port in ports {
        let processed = process_port(&port.cat, &port.name, &port.port_type)?;
        cpp_ports.push(processed.cpp_port);
        cpp_pragmas.push(processed.cpp_pragma);
    }

    let ports_str = cpp_ports.join(", ");
    let pragma_body = cpp_pragmas.join("\n");

    // slot_def.j2: void {{ name }}({{ ports }}) { {{ pragma }} }
    let new_def = format!("void {slot_name}({ports_str}) {{\n    {pragma_body}\n}}");
    // slot_decl.j2: void {{ name }}({{ ports }});
    let new_decl = format!("void {slot_name}({ports_str});");

    cpp_surgery::replace_function(top_cpp, top_name, &new_decl, Some(&new_def))
}
