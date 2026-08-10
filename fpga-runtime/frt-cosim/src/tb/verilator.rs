use askama::Template;
use std::collections::HashMap;

use crate::error::{CosimError, Result};
use crate::metadata::{ArgKind, KernelSpec, Mode};
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
        let mode_str = match self.spec.mode {
            Mode::Hls => "hls",
            Mode::Vitis => "vitis",
        };

        let verilog_contents = read_verilog_contents(self.spec);
        let base_addresses = self.base_addresses;
        let buffer_sizes = self.buffer_sizes;

        let (mmap_args, scalar_args, stream_args, stream_out_args) = classify_args(
            self.spec,
            self.scalar_values,
            &verilog_contents,
            |arg, offset| {
                let data_width = match &arg.kind {
                    ArgKind::Mmap { data_width, .. } => *data_width,
                    ArgKind::Scalar { .. } | ArgKind::Stream { .. } => unreachable!(),
                };
                let offset_port = super::direct_offset_port_name(&verilog_contents, &arg.name);
                MmapArg {
                    // The context creates a segment for every mmap argument,
                    // so the entry is always here in production. An unknown
                    // size models nothing rather than guessing at one: reads
                    // return zero and writes are dropped, which is what an
                    // unbound argument did when the model was a hash map.
                    data_size: buffer_sizes.get(&arg.name).copied().unwrap_or(0),
                    ..MmapArg::new(
                        NAMING,
                        &arg.name,
                        &offset_port,
                        (data_width as usize).div_ceil(8),
                        base_addresses.get(&arg.name).copied().unwrap_or(0),
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
            |arg, width_bytes, peek, axis| {
                StreamArg::new(NAMING, &arg.name, width_bytes, peek, axis)
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
