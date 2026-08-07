use super::{kernel_xml, KernelSpec, Mode};
use crate::error::Result;
use std::collections::HashMap;
use std::path::Path;

/// Read an `.xo`/`.zip` `kernel.xml` into a [`KernelSpec`]. The XML itself is
/// parsed by [`kernel_xml`], which the xclbin path shares; everything else on
/// a `KernelSpec` comes from the package around it.
pub fn parse_kernel_xml(xml: &str, _verilog_dir: &Path) -> Result<KernelSpec> {
    let parsed = kernel_xml::parse(xml)?;
    Ok(KernelSpec {
        top_name: parsed.top_name,
        mode: Mode::Vitis,
        args: parsed.args,
        part_num: None,
        verilog_files: vec![],
        tcl_files: vec![],
        xci_files: vec![],
        scalar_register_map: HashMap::new(),
    })
}
