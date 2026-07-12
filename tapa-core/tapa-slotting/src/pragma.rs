//! Port category to HLS pragma and C++ port template mapping.

use regex::Regex;
use std::sync::LazyLock;

use crate::error::SlottingError;

// ── Pragma templates ─────────────────────────────────────────────────

fn render_pragma(template_name: &str, name: &str, port_type: Option<&str>) -> String {
    let mut env = minijinja::Environment::new();
    let source = match template_name {
        "scalar" => include_str!("templates/scalar_pragma.cpp.j2"),
        "mmap" => include_str!("templates/mmap_pragma.cpp.j2"),
        "fifo_in" => include_str!("templates/fifo_in_pragma.cpp.j2"),
        "fifo_out" => include_str!("templates/fifo_out_pragma.cpp.j2"),
        _ => unreachable!(),
    };
    env.add_template(template_name, source)
        .expect("template parses");
    let ctx = if let Some(pt) = port_type {
        minijinja::context! { name, port_type => pt }
    } else {
        minijinja::context! { name }
    };
    env.get_template(template_name)
        .expect("template exists")
        .render(ctx)
        .expect("render succeeds")
}

fn scalar_pragma(name: &str) -> String {
    render_pragma("scalar", name, None)
}

fn mmap_pragma(name: &str) -> String {
    render_pragma("mmap", name, None)
}

fn fifo_in_pragma(name: &str) -> String {
    render_pragma("fifo_in", name, None)
}

fn fifo_out_pragma(name: &str, port_type: &str) -> String {
    render_pragma("fifo_out", name, Some(port_type))
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
    LazyLock::new(|| Regex::new(r"^([a-zA-Z_]\w*)\[(\d+)\]([a-zA-Z_]\w*)?$").unwrap());

static SCALAR_TYPE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:tapa::)?(\w+)<([^,>]+)").unwrap());

/// A processed port ready for C++ emission.
pub struct ProcessedPort {
    pub cpp_port: String,
    pub cpp_pragma: String,
}

/// Process a single port dict into C++ port declaration and HLS pragma.
///
/// `cat`: port category (istream, ostream, scalar, mmap, immap, ommap,
/// `async_mmap`, hmap, istreams, ostreams)
/// `name`: port name, possibly with array index like `port[0]`
/// `port_type`: C++ type string
pub fn process_port(
    cat: &str,
    name: &str,
    port_type: &str,
) -> Result<ProcessedPort, SlottingError> {
    // Normalize indexed names: port[0] -> port_0, port[0]_inst -> port_0_inst
    let normalized_name = if let Some(caps) = INDEXED_PORT_RE.captures(name) {
        format!(
            "{}_{}{}",
            &caps[1],
            &caps[2],
            caps.get(3).map_or("", |m| m.as_str())
        )
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
        "mmap" | "immap" | "ommap" | "async_mmap" => {
            format!("{effective_type} {normalized_name}_offset")
        }
        "istream" | "ostream" | "istreams" | "ostreams" => {
            stream_port(&effective_cat, &effective_type, &normalized_name)
        }
        _ => return Err(SlottingError::UnknownPortCategory(effective_cat)),
    };

    let cpp_pragma = match effective_cat.as_str() {
        "scalar" | "hmap" => scalar_pragma(&normalized_name),
        "mmap" | "immap" | "ommap" | "async_mmap" => mmap_pragma(&normalized_name),
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
    fn directional_mmap_ports() {
        for cat in ["immap", "ommap"] {
            let p = process_port(cat, "mem", "uint64_t").unwrap();
            assert_eq!(p.cpp_port, "uint64_t mem_offset");
            assert!(p.cpp_pragma.contains("mem_offset register"));
        }
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
    fn indexed_port_with_suffix() {
        let p = process_port("ostream", "qs[24]_Network", "int").unwrap();
        assert_eq!(p.cpp_port, "tapa::ostream<int>& qs_24_Network");
        assert!(p.cpp_pragma.contains("ap_fifo port = qs_24_Network._"));
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
