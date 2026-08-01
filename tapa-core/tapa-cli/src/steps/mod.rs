//! Subcommand handlers — one module per CLI command.
//!
//! Each module exposes a clap `Args` struct (named `<Step>Args` to keep
//! clap's argument-group names unique under flatten) and a
//! `run(&args, ctx)` entry point. The dispatcher in `chain.rs` wires
//! each module up through clap's `Subcommand` derive.
//!
//! # Vendor/backend seam
//!
//! TAPA currently targets a single vendor (Xilinx), but the pieces that would
//! differ per backend are kept behind clear boundaries so a second vendor can
//! be added without threading changes through the whole CLI:
//!
//! - **Flow selection** — `tapa_ir::TaskGraph::target` is the single home of
//!   the compilation flow, typed as `tapa_ir::Target` and validated when the
//!   state file parses. `pack` dispatches on it with an exhaustive `match`,
//!   so adding a `Target` variant makes the compiler flag every dispatch
//!   site.
//! - **RTL codegen** — `tapa_codegen::top_stream_needs_axis_adapter` is the
//!   single function in codegen that branches on `Target` (exhaustive
//!   `match`); a new variant is a compile error there. When a second vendor
//!   needs more than this one decision, promote it to a `Backend` trait.
//! - **Synthesis** lives in `synth/{device_resolve,hls_run}` (Vitis HLS
//!   today); a second vendor would add a parallel synth path.
//! - **Packaging** lives in `pack/{vitis_packaging,kernel_xml_ports}`.
//! - The vendor toolchain itself is the `tapa-xilinx` crate.

pub mod analyze;
pub mod floorplan;
pub mod gcc;
pub mod meta;
pub mod pack;
pub(crate) mod registry;
pub mod synth;
pub mod version;
