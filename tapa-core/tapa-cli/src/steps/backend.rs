//! Vendor/backend seam.
//!
//! TAPA currently targets a single vendor (Xilinx), but the pieces that would
//! differ per backend are kept behind clear boundaries so a second vendor can
//! be added without threading changes through the whole CLI:
//!
//! - **Flow selection** — [`effective_target`] resolves the compilation flow
//!   ([`Target`]); `pack` dispatches on it with an exhaustive `match`, so
//!   adding a `Target` variant makes the compiler flag every dispatch site.
//! - **Synthesis** lives in `steps/synth/{device_resolve,hls_run}` (Vitis HLS
//!   today); a second vendor would add a parallel synth path.
//! - **Packaging** lives in `steps/pack/{vitis_packaging,kernel_xml_ports}`.
//! - The vendor toolchain itself is the `tapa-xilinx` crate.

use std::str::FromStr;

use serde_json::Value;
use tapa_ir::{Design, Target};

use crate::error::{CliError, Result};
use crate::state::settings::Settings;

/// Resolve the effective compilation flow target.
///
/// `settings.json` holds `target` as an untyped string that may drift from
/// `design.target`; when present it wins (an unrecognized value is a hard
/// error), otherwise the already-typed `design.target` is used. This is the
/// single place the CLI decides which backend flow a run drives.
pub fn effective_target(settings: &Settings, design: &Design) -> Result<Target> {
    match settings.get("target").and_then(Value::as_str) {
        Some(s) => Target::from_str(s).map_err(|_| {
            CliError::InvalidArg(format!(
                "unsupported target `{s}` in settings.json; \
                 supported flows are `xilinx-vitis` and `xilinx-hls`"
            ))
        }),
        None => Ok(design.target),
    }
}
