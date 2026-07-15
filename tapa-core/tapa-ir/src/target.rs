//! Compilation flow target.

use std::str::FromStr;

use serde::{Deserialize, Serialize};
use strum::EnumString;

/// Compilation flow target — the two flows the pipeline can drive.
///
/// Serializes to the exact wire strings `"xilinx-vitis"` / `"xilinx-hls"`
/// used in `design.json`'s `target` field and echoed into `settings.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum Target {
    XilinxVitis,
    XilinxHls,
}

impl Target {
    /// Canonical wire string (`"xilinx-vitis"` / `"xilinx-hls"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::XilinxVitis => "xilinx-vitis",
            Self::XilinxHls => "xilinx-hls",
        }
    }
}

impl Serialize for Target {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Target {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(|_| serde::de::Error::custom(format!("unknown target: {s}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xilinx_vitis_round_trips() {
        let json = serde_json::to_string(&Target::XilinxVitis).expect("serialize");
        assert_eq!(json, r#""xilinx-vitis""#);
        let back: Target = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, Target::XilinxVitis);
    }

    #[test]
    fn xilinx_hls_round_trips() {
        let json = serde_json::to_string(&Target::XilinxHls).expect("serialize");
        assert_eq!(json, r#""xilinx-hls""#);
        let back: Target = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, Target::XilinxHls);
    }

    #[test]
    fn as_str_matches_wire_form() {
        assert_eq!(Target::XilinxVitis.as_str(), "xilinx-vitis");
        assert_eq!(Target::XilinxHls.as_str(), "xilinx-hls");
    }

    #[test]
    fn from_str_parses_wire_form() {
        assert_eq!(
            Target::from_str("xilinx-vitis").expect("parse"),
            Target::XilinxVitis,
        );
        assert_eq!(
            Target::from_str("xilinx-hls").expect("parse"),
            Target::XilinxHls,
        );
    }

    #[test]
    fn unknown_value_rejected() {
        assert!(
            serde_json::from_str::<Target>(r#""cpu-sim""#).is_err(),
            "unknown target must be rejected",
        );
        assert!(
            Target::from_str("cpu-sim").is_err(),
            "unknown target must be rejected by FromStr",
        );
    }
}
