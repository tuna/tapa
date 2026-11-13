use super::kernel_xml::{self, KernelXml};
use super::{register_offset, ArgKind, KernelSpec, Mode};
use crate::error::{CosimError, Result};
use std::collections::HashMap;

/// Read an `.xo`/`.zip` `kernel.xml` into a [`KernelSpec`]. The XML itself is
/// parsed by [`kernel_xml`], which the xclbin path shares; everything else on
/// a `KernelSpec` comes from the package around it.
pub fn parse_kernel_xml(xml: &str) -> Result<KernelSpec> {
    kernel_xml::parse(xml).map(spec)
}

pub(super) fn spec(parsed: KernelXml) -> KernelSpec {
    KernelSpec {
        top_name: parsed.top_name,
        mode: Mode::Vitis,
        args: parsed.args,
        part_num: None,
        verilog_files: vec![],
        tcl_files: vec![],
        xci_files: vec![],
        scalar_register_map: HashMap::new(),
        inlined: Vec::new(),
        memories: Vec::new(),
        rings: Vec::new(),
    }
}

/// On a card XRT writes each argument at the offset `kernel.xml` gives it,
/// while the testbench writes it where the control slave decodes it, so a
/// package whose two disagree would pass cosimulation and fail on hardware.
/// Only what both state is compared: a stream has no register, a foreign
/// `kernel.xml` may give no offsets, and a hand-written slave may not name
/// every argument.
pub(super) fn check_register_offsets(
    parsed: &KernelXml,
    slave: &HashMap<String, u32>,
) -> Result<()> {
    for arg in &parsed.args {
        if matches!(arg.kind, ArgKind::Stream { .. }) {
            continue;
        }
        let (Some(&declared), Some(decoded)) = (
            parsed.offsets.get(&arg.id),
            register_offset(slave, &arg.name),
        ) else {
            continue;
        };
        if declared != decoded {
            return Err(CosimError::Metadata(format!(
                "kernel.xml places arg {:?} at register offset {declared:#x}, but the control \
                 slave decodes it at {decoded:#x}: on hardware XRT would write it to the wrong \
                 register",
                arg.name
            )));
        }
    }
    Ok(())
}
