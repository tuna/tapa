//! Per-task synthesis target.

use serde::{Deserialize, Serialize};

/// Per-task synthesis target from `graph.json` / `design.json`.
///
/// The pipeline only distinguishes `Ignore` (skip synthesis) from everything
/// else (synthesize). `tapacc` emits `"hls"` for HLS leaf tasks and `"ignore"`
/// for skipped ones, but a task (notably the top task) can also carry a
/// flow-derived value such as `"xilinx_vitis"`. Any unrecognized string is
/// preserved verbatim in [`SynthTarget::Other`] so the wire form round-trips.
/// Distinct from the flow-level [`crate::Target`] (`"xilinx-vitis"` / `"xilinx-hls"`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SynthTarget {
    /// `"hls"` — synthesize with Vitis HLS.
    Hls,
    /// `"ignore"` — skip synthesis (custom-RTL / passthrough tasks).
    Ignore,
    /// Any other wire value, preserved verbatim.
    Other(String),
}

impl SynthTarget {
    /// Canonical wire string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Hls => "hls",
            Self::Ignore => "ignore",
            Self::Other(other) => other,
        }
    }
}

impl From<String> for SynthTarget {
    fn from(value: String) -> Self {
        match value.as_str() {
            "hls" => Self::Hls,
            "ignore" => Self::Ignore,
            _ => Self::Other(value),
        }
    }
}

impl Serialize for SynthTarget {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SynthTarget {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from(String::deserialize(deserializer)?))
    }
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
    fn as_str_matches_wire_form() {
        assert_eq!(SynthTarget::Hls.as_str(), "hls");
        assert_eq!(SynthTarget::Ignore.as_str(), "ignore");
    }

    #[test]
    fn unknown_value_is_preserved_verbatim() {
        // `tapacc` can emit a flow-derived task target (e.g. the top task's
        // `xilinx_vitis`); it must round-trip byte-identically, not be rejected.
        let back: SynthTarget =
            serde_json::from_str(r#""xilinx_vitis""#).expect("unknown target is accepted");
        assert_eq!(back, SynthTarget::Other("xilinx_vitis".to_string()));
        assert_eq!(back.as_str(), "xilinx_vitis");
        assert_eq!(serde_json::to_string(&back).unwrap(), r#""xilinx_vitis""#);
    }
}
