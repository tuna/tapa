use askama::Template;
use std::collections::HashMap;

use crate::error::{CosimError, Result};
use crate::metadata::KernelSpec;
use crate::tb::names::cpp_identifier;
use crate::tb::{classify_args, read_verilog_contents, MmapArg, Naming, ScalarArg, StreamArg};

/// Verilator exposes every DUT port as a C++ member, so both kinds of name
/// are the same mangling.
const NAMING: Naming = Naming {
    ident: cpp_identifier,
    port: cpp_identifier,
};

#[derive(Template)]
#[template(path = "tb_verilator.cpp.j2", escape = "none")]
struct TbTemplate<'a> {
    top_name: &'a str,
    mode: &'a str,
    mmap_args: Vec<MmapArg>,
    scalar_args: Vec<ScalarArg>,
    stream_args: Vec<StreamArg>,
    stream_out_args: Vec<StreamArg>,
}

pub struct VerilatorTbGenerator<'a> {
    spec: &'a KernelSpec,
    base_addresses: &'a HashMap<String, u64>,
    buffer_sizes: &'a HashMap<String, usize>,
    scalar_values: &'a HashMap<u32, Vec<u8>>,
}

impl<'a> VerilatorTbGenerator<'a> {
    pub fn new(
        spec: &'a KernelSpec,
        base_addresses: &'a HashMap<String, u64>,
        buffer_sizes: &'a HashMap<String, usize>,
        scalar_values: &'a HashMap<u32, Vec<u8>>,
    ) -> Self {
        Self {
            spec,
            base_addresses,
            buffer_sizes,
            scalar_values,
        }
    }

    pub fn render_tb(&self) -> Result<String> {
        let mode_str = self.spec.mode.as_str();

        let verilog_contents = read_verilog_contents(self.spec);
        let base_addresses = self.base_addresses;
        let buffer_sizes = self.buffer_sizes;

        let (mmap_args, scalar_args, stream_args, stream_out_args) = classify_args(
            self.spec,
            self.scalar_values,
            &verilog_contents,
            |arg, offset| {
                MmapArg {
                    // The context creates a segment for every mmap argument,
                    // so the entry is always here in production. An unknown
                    // size models nothing rather than guessing at one: reads
                    // return zero and writes are dropped, which is what an
                    // unbound argument did when the model was a hash map.
                    data_size: buffer_sizes.get(&arg.name).copied().unwrap_or(0),
                    ..super::mmap_arg_from_spec(
                        NAMING,
                        arg,
                        &verilog_contents,
                        base_addresses,
                        offset,
                    )
                }
            },
            |arg, _width, offset, bytes| {
                ScalarArg::new(
                    NAMING,
                    &arg.name,
                    bytes_to_cpp_initializer(bytes),
                    offset,
                    bytes,
                )
            },
            |arg, width_bytes, bits, peek, axis| {
                StreamArg::new(NAMING, &arg.name, width_bytes, bits, peek, axis)
            },
        )?;

        let tmpl = TbTemplate {
            top_name: &self.spec.top_name,
            mode: mode_str,
            mmap_args,
            scalar_args,
            stream_args,
            stream_out_args,
        };
        tmpl.render()
            .map_err(|e| CosimError::Metadata(format!("template render failed: {e}")))
    }
}

fn bytes_to_cpp_initializer(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("0x{b:02x}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{ArgKind, ArgSpec, Mode};

    #[test]
    fn control_write_address_does_not_truncate_large_offsets() {
        let spec = KernelSpec {
            top_name: "Top".to_owned(),
            mode: Mode::Vitis,
            args: vec![ArgSpec {
                name: "value".to_owned(),
                id: 0,
                kind: ArgKind::Scalar { width: 32 },
            }],
            part_num: None,
            verilog_files: vec![],
            tcl_files: vec![],
            xci_files: vec![],
            scalar_register_map: HashMap::from([("value".to_owned(), 0x100)]),
        };
        let base_addresses = HashMap::new();
        let buffer_sizes = HashMap::new();
        let scalar_values = HashMap::from([(0, vec![0; 4])]);
        let tb = VerilatorTbGenerator::new(&spec, &base_addresses, &buffer_sizes, &scalar_values)
            .render_tb()
            .expect("render testbench");

        assert!(tb.contains("static void ctrl_write(uint32_t addr"));
        assert!(tb.contains("ctrl_write(0x100"));
    }
}
