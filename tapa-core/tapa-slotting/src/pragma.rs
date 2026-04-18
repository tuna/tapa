//! Port category to HLS pragma and C++ port template mapping.

use regex::Regex;
use std::sync::LazyLock;

use crate::error::SlottingError;

// ── Pragma templates ─────────────────────────────────────────────────

fn scalar_pragma(name: &str) -> String {
    format!(
        "#pragma HLS interface ap_none port = {name} register\n\
         {{ auto val = reinterpret_cast<volatile uint8_t &>({name}); }}"
    )
}

fn mmap_pragma(name: &str) -> String {
    format!(
        "#pragma HLS interface ap_none port = {name}_offset register\n\
         {{ auto val = reinterpret_cast<volatile uint8_t &>({name}_offset); }}"
    )
}

fn fifo_in_pragma(name: &str) -> String {
    format!(
        "#pragma HLS disaggregate variable = {name}\n\
         #pragma HLS interface ap_fifo port = {name}._\n\
         #pragma HLS aggregate variable = {name}._ bit\n\
         void({name}._.empty());\n\
         {{ auto val = {name}.read(); }}\n"
    )
}

fn fifo_out_pragma(name: &str, port_type: &str) -> String {
    format!(
        "#pragma HLS disaggregate variable = {name}\n\
         #pragma HLS interface ap_fifo port = {name}._\n\
         #pragma HLS aggregate variable = {name}._ bit\n\
         void({name}._.full());\n\
         {name}.write({port_type}());"
    )
}

// ── Port templates ───────────────────────────────────────────────────

fn stream_port(cat: &str, port_type: &str, name: &str) -> String {
    let stream_kind = match cat {
        "istream" | "istreams" => "istream",
        "ostream" | "ostreams" => "ostream",
        _ => unreachable!(),
    };
    format!("tapa::{stream_kind}<{port_type}>& {name}")
}

// ── Port processing ──────────────────────────────────────────────────

static INDEXED_PORT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([a-zA-Z_]\w*)\[(\d+)\]$").unwrap());

static SCALAR_TYPE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:tapa::)?(\w+)<([^,>]+)").unwrap());

/// A processed port ready for C++ emission.
pub struct ProcessedPort {
    pub cpp_port: String,
    pub cpp_pragma: String,
}

/// Process a single port dict into C++ port declaration and HLS pragma.
///
/// `cat`: port category (istream, ostream, scalar, mmap, `async_mmap`, hmap, istreams, ostreams)
/// `name`: port name, possibly with array index like `port[0]`
/// `port_type`: C++ type string
pub fn process_port(
    cat: &str,
    name: &str,
    port_type: &str,
) -> Result<ProcessedPort, SlottingError> {
    // Normalize indexed names: port[0] -> port_0
    let normalized_name = if let Some(caps) = INDEXED_PORT_RE.captures(name) {
        format!("{}_{}", &caps[1], &caps[2])
    } else if name.contains('[') {
        return Err(SlottingError::InvalidPortIndex(name.to_owned()));
    } else {
        name.to_owned()
    };

    // Fix scalar category for stream/mmap types
    let mut effective_cat = cat.to_owned();
    let mut effective_type = port_type.to_owned();

    if effective_cat == "scalar" {
        if let Some(caps) = SCALAR_TYPE_RE.captures(port_type) {
            caps[1].clone_into(&mut effective_cat);
            caps[2].clone_into(&mut effective_type);
        }
    }

    // Pointer types -> uint64_t
    if effective_type.contains('*') {
        "uint64_t".clone_into(&mut effective_type);
    }
    // Strip const prefix
    if let Some(stripped) = effective_type.strip_prefix("const ") {
        effective_type = stripped.to_owned();
    }

    let cpp_port = match effective_cat.as_str() {
        "scalar" | "hmap" => format!("{effective_type} {normalized_name}"),
        "mmap" | "async_mmap" => format!("{effective_type} {normalized_name}_offset"),
        "istream" | "ostream" | "istreams" | "ostreams" => {
            stream_port(&effective_cat, &effective_type, &normalized_name)
        }
        _ => return Err(SlottingError::UnknownPortCategory(effective_cat)),
    };

    let cpp_pragma = match effective_cat.as_str() {
        "scalar" | "hmap" => scalar_pragma(&normalized_name),
        "mmap" | "async_mmap" => mmap_pragma(&normalized_name),
        "istream" | "istreams" => fifo_in_pragma(&normalized_name),
        "ostream" | "ostreams" => fifo_out_pragma(&normalized_name, &effective_type),
        _ => return Err(SlottingError::UnknownPortCategory(effective_cat)),
    };

    Ok(ProcessedPort {
        cpp_port,
        cpp_pragma,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_port() {
        let p = process_port("scalar", "x", "int").unwrap();
        assert_eq!(p.cpp_port, "int x");
        assert!(p.cpp_pragma.contains("ap_none port = x register"));
    }

    #[test]
    fn mmap_port() {
        let p = process_port("mmap", "addr", "uint64_t").unwrap();
        assert_eq!(p.cpp_port, "uint64_t addr_offset");
        assert!(p.cpp_pragma.contains("addr_offset register"));
    }

    #[test]
    fn async_mmap_port() {
        let p = process_port("async_mmap", "mem", "uint64_t").unwrap();
        assert_eq!(p.cpp_port, "uint64_t mem_offset");
    }

    #[test]
    fn istream_port() {
        let p = process_port("istream", "in_data", "float").unwrap();
        assert_eq!(p.cpp_port, "tapa::istream<float>& in_data");
        assert!(p.cpp_pragma.contains("ap_fifo port = in_data._"));
    }

    #[test]
    fn ostream_port() {
        let p = process_port("ostream", "out_data", "float").unwrap();
        assert_eq!(p.cpp_port, "tapa::ostream<float>& out_data");
        assert!(p.cpp_pragma.contains("out_data.write(float())"));
    }

    #[test]
    fn indexed_port() {
        let p = process_port("scalar", "arr[10]", "int").unwrap();
        assert_eq!(p.cpp_port, "int arr_10");
    }

    #[test]
    fn invalid_index() {
        let result = process_port("scalar", "arr[x]", "int");
        assert!(result.is_err());
    }

    #[test]
    fn scalar_with_stream_type() {
        let p = process_port("scalar", "data", "tapa::istream<float>").unwrap();
        assert_eq!(p.cpp_port, "tapa::istream<float>& data");
    }

    #[test]
    fn pointer_type_to_uint64() {
        let p = process_port("scalar", "ptr", "int*").unwrap();
        assert_eq!(p.cpp_port, "uint64_t ptr");
    }

    #[test]
    fn const_type_stripped() {
        let p = process_port("scalar", "val", "const int").unwrap();
        assert_eq!(p.cpp_port, "int val");
    }

    #[test]
    fn hmap_port() {
        let p = process_port("hmap", "mem", "uint64_t").unwrap();
        assert_eq!(p.cpp_port, "uint64_t mem");
        assert!(p.cpp_pragma.contains("ap_none port = mem register"));
    }
}
