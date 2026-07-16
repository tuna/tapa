//! Per-task synthesis target.

use std::str::FromStr;

use serde::{Deserialize, Serialize};
use strum::EnumString;

/// Per-task synthesis target — the two values a task can carry.
///
/// Serializes to the exact wire strings `"hls"` / `"ignore"` used in the
/// per-task `target` field of `graph.json` and `design.json`. Distinct from
/// the flow-level [`crate::Target`] (`"xilinx-vitis"` / `"xilinx-hls"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum SynthTarget {
    Hls,
    Ignore,
}

impl SynthTarget {
    /// Canonical wire string (`"hls"` / `"ignore"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hls => "hls",
            Self::Ignore => "ignore",
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
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s)
            .map_err(|_| serde::de::Error::custom(format!("unknown synth target: {s}")))
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
    fn from_str_parses_wire_form() {
        assert_eq!(
            SynthTarget::from_str("hls").expect("parse"),
            SynthTarget::Hls
        );
        assert_eq!(
            SynthTarget::from_str("ignore").expect("parse"),
            SynthTarget::Ignore,
        );
    }

    #[test]
    fn unknown_value_rejected() {
        assert!(
            serde_json::from_str::<SynthTarget>(r#""cosim""#).is_err(),
            "unknown synth target must be rejected",
        );
        assert!(
            SynthTarget::from_str("cosim").is_err(),
            "unknown synth target must be rejected by FromStr",
        );
    }
}
