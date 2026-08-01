// Workspace lints enforce pedantic+nursery+restriction; the allows below
// cover idiomatic-in-this-crate patterns that don't add reader value.
#![allow(
    clippy::too_many_arguments,
    reason = "kernel XML argument records carry many schema fields"
)]
#![allow(
    clippy::doc_markdown,
    reason = "doc comments include tool names and file-like identifiers"
)]
#![allow(
    clippy::assigning_clones,
    reason = "tool-runner builders assemble Vecs then pass through"
)]
#![allow(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "Xilinx archive extensions are always lowercase on disk"
)]
#![allow(
    clippy::cast_possible_truncation,
    reason = "ZIP entry sizes always fit in usize on target platforms"
)]
#![allow(
    clippy::redundant_closure_for_method_calls,
    reason = "clearer at the Err(...) call site"
)]
#![allow(
    clippy::semicolon_outside_block,
    reason = "scoped writer blocks keep lifetimes tight"
)]
#![allow(
    clippy::too_long_first_doc_paragraph,
    reason = "module docs summarize several Xilinx tool responsibilities"
)]
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "test assertions on error kinds use _ intentionally"
)]

//! Rust wrappers around the Xilinx toolchain: Vitis HLS, Vivado, `.xo`
//! packaging, platform XML parsing, and remote SSH execution.
//!
//! Layout: `runtime/` provides infrastructure (tool discovery, CFLAGS,
//! remote config, subprocess abstraction, SSH control-master session,
//! remote tool runner); `platform/` owns pure parsers and emitters
//! (`.xpfm`, `kernel.xml`); `tools/` composes runtime + platform into
//! tool orchestrators (HLS, Vivado, `.xo` packaging). Dependency
//! direction is strictly `runtime/` ← `platform/` ← `tools/`.

pub mod connectivity;
pub mod error;
pub mod platform;
pub mod runtime;
pub mod tools;
mod util;

pub use error::{Result, XilinxError};
pub use platform::device::{parse_device_info, parse_hpfm_xml, parse_xpfm, DeviceInfo};
pub use platform::kernel_xml::{emit_kernel_xml, KernelXmlArgs, KernelXmlPort, PortCategory};
pub use runtime::config::RemoteConfig;
pub use runtime::process::{
    LocalToolRunner, MockToolRunner, ToolInvocation, ToolOutput, ToolRunner,
};
pub use runtime::remote::RemoteToolRunner;
pub use runtime::ssh::{SshMuxOptions, SshSession};
pub use runtime::vendor::sync_remote_vendor_includes;
pub use tools::hls::report::{
    parse_csynth_xml, parse_utilization_rpt, CsynthReport, UtilizationReport,
};
pub use tools::hls::{
    build_hls_tcl, run_hls_with_retry, run_hls_with_retry_in_stage, HlsJob, HlsOutput,
};
pub use tools::package_xo::{pack_xo, pack_xo_without_redaction, redact_xo, PackageXoInputs};
pub use tools::vitis::timing::{parse_kernel_timing_summary, KernelTiming};
pub use tools::vitis::{run_vitis_link, target_frequency_mhz, VitisLinkJob, VitisLinkOutput};
pub use tools::vivado::{run_vivado, VivadoJob, VivadoOutput};
