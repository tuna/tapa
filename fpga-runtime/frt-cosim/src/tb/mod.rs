pub mod names;
pub mod verilator;
pub mod xsim;

use std::collections::HashMap;

use crate::metadata::KernelSpec;

#[derive(Clone)]
pub struct ScalarWord {
    pub reg_offset: u32,
    pub value_u32: u32,
}

/// Look up the register offset for an arg name in the scalar register map.
/// Falls back to `"{name}_offset"` if the exact name is not found.
pub fn lookup_register_offset(spec: &KernelSpec, name: &str) -> u32 {
    spec.scalar_register_map
        .get(name)
        .or_else(|| {
            let key = format!("{name}_offset");
            spec.scalar_register_map.get(&key)
        })
        .copied()
        .unwrap_or(0)
}

/// Read all Verilog file contents for peek-port detection and similar scans.
pub fn read_verilog_contents(spec: &KernelSpec) -> Vec<String> {
    spec.verilog_files
        .iter()
        .filter_map(|f| std::fs::read_to_string(f).ok())
        .collect()
}

/// Classify spec args into four groups by applying backend-specific constructors.
#[allow(
    clippy::implicit_hasher,
    reason = "generic hasher adds no value for internal helper"
)]
pub fn classify_args<M, S, T>(
    spec: &KernelSpec,
    scalar_values: &HashMap<u32, Vec<u8>>,
    verilog_contents: &[String],
    make_mmap: impl Fn(&crate::metadata::ArgSpec, u32) -> M,
    make_scalar: impl Fn(&crate::metadata::ArgSpec, u32, u32, &[u8]) -> S,
    make_stream: impl Fn(&crate::metadata::ArgSpec, usize, Option<String>, bool) -> T,
) -> (Vec<M>, Vec<S>, Vec<T>, Vec<T>) {
    use crate::metadata::{ArgKind, Mode, StreamDir, StreamProtocol};
    use names::{infer_peek_name, stream_peek_ports_exist};

    let mut mmaps = vec![];
    let mut scalars = vec![];
    let mut streams_in = vec![];
    let mut streams_out = vec![];

    for arg in &spec.args {
        match &arg.kind {
            ArgKind::Mmap { .. } => {
                let offset = lookup_register_offset(spec, &arg.name);
                mmaps.push(make_mmap(arg, offset));
            }
            ArgKind::Scalar { width } => {
                let offset = lookup_register_offset(spec, &arg.name);
                let bytes =
                    normalized_scalar_bytes(*width, scalar_values.get(&arg.id).map(Vec::as_slice));
                scalars.push(make_scalar(arg, *width, offset, &bytes));
            }
            ArgKind::Stream {
                width,
                dir,
                protocol,
                ..
            } => {
                let w = (*width as usize).div_ceil(8);
                let axis = *protocol == StreamProtocol::Axis;
                let peek = if spec.mode == Mode::Hls && *dir == StreamDir::In {
                    infer_peek_name(&arg.name).filter(|cand| {
                        stream_peek_ports_exist(verilog_contents, &spec.top_name, cand)
                    })
                } else {
                    None
                };
                let s = make_stream(arg, w, peek, axis);
                match dir {
                    StreamDir::In => streams_in.push(s),
                    StreamDir::Out => streams_out.push(s),
                }
            }
        }
    }
    (mmaps, scalars, streams_in, streams_out)
}

pub fn normalized_scalar_bytes(width_bits: u32, raw: Option<&[u8]>) -> Vec<u8> {
    let expected = (width_bits as usize).div_ceil(8).max(1);
    let mut out = raw.map(<[u8]>::to_vec).unwrap_or_default();
    if out.len() < expected {
        out.resize(expected, 0);
    } else if out.len() > expected {
        out.truncate(expected);
    }
    out
}

pub fn scalar_words(base_offset: u32, bytes: &[u8]) -> Vec<ScalarWord> {
    let mut words = Vec::new();
    for (i, chunk) in bytes.chunks(4).enumerate() {
        let mut raw = [0u8; 4];
        raw[..chunk.len()].copy_from_slice(chunk);
        words.push(ScalarWord {
            reg_offset: base_offset + (i as u32) * 4,
            value_u32: u32::from_le_bytes(raw),
        });
    }
    if words.is_empty() {
        words.push(ScalarWord {
            reg_offset: base_offset,
            value_u32: 0,
        });
    }
    words
}
