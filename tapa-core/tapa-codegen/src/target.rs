//! Vendor-flow codegen policy.

use tapa_ir::Target;

/// Vendor-flow codegen policy.
///
/// This is the **single place** in `tapa-codegen` that branches on the
/// vendor flow ([`Target`]). Today only one decision differs across
/// vendors: whether the top task's external stream FIFOs need a
/// Vitis-style AXIS adapter at the module boundary. The exhaustive
/// `match` makes adding a [`Target`] variant a compile error here.
///
/// When a second vendor needs more than this one boolean, promote this
/// to a `Backend` trait implemented per vendor (the trait surface would
/// then be shaped against the real second vendor's codegen deltas, per
/// the "shape against a real vendor" principle).
#[must_use]
pub fn top_stream_needs_axis_adapter(target: Target) -> bool {
    match target {
        Target::XilinxVitis => true,
        Target::XilinxHls => false,
    }
}
