//! Per-task synthesis policy.

use serde::{Deserialize, Serialize};

/// Per-task synthesis policy.
///
/// The pipeline only distinguishes `Ignore` (skip synthesis) from `Hls`
/// (synthesize with Vitis HLS). Serializes as the wire strings `"hls"` /
/// `"ignore"`; any other value is a hard deserialize error. Distinct from
/// the flow-level [`crate::Target`] (`"xilinx-vitis"` / `"xilinx-hls"`),
/// which lives once at the graph root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SynthTarget {
    /// `"hls"` — synthesize with Vitis HLS.
    Hls,
    /// `"ignore"` — skip synthesis (custom-RTL / passthrough tasks).
    Ignore,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hls_round_trips() {
        let json = serde_json::to_string(&SynthTarget::Hls).expect("serialize");
        assert_eq!(json, r#""hls""#);
        let back: SynthTarget = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, SynthTarget::Hls);
    }

    #[test]
    fn ignore_round_trips() {
        let json = serde_json::to_string(&SynthTarget::Ignore).expect("serialize");
        assert_eq!(json, r#""ignore""#);
        let back: SynthTarget = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, SynthTarget::Ignore);
    }

    #[test]
    fn unknown_value_rejected() {
        // The old flow-derived task target (e.g. `"xilinx_vitis"`) is no
        // longer a valid per-task synth policy: it must be rejected, not
        // preserved. tapacc emits `"hls"` / `"ignore"` only.
        assert!(
            serde_json::from_str::<SynthTarget>(r#""xilinx_vitis""#).is_err(),
            "unknown synth policy must be rejected",
        );
    }
}
