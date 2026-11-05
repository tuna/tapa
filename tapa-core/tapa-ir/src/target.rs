//! Compilation flow target.

use std::str::FromStr;

use serde::{Deserialize, Serialize};
use strum::EnumString;

/// Compilation flow target — the flows the pipeline can drive.
///
/// Serializes to the exact wire strings `"xilinx-vitis"` / `"xilinx-slash"` /
/// `"xilinx-hls"`.
/// [`crate::TaskGraph::target`] is its single home — it is deliberately not
/// duplicated into the flow settings block alongside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString, Serialize)]
#[strum(serialize_all = "kebab-case")]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    XilinxVitis,
    XilinxSlash,
    XilinxHls,
}

impl Target {
    /// Canonical wire string (`"xilinx-vitis"` / `"xilinx-slash"` / `"xilinx-hls"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::XilinxVitis => "xilinx-vitis",
            Self::XilinxSlash => "xilinx-slash",
            Self::XilinxHls => "xilinx-hls",
        }
    }
}

impl<'de> Deserialize<'de> for Target {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(|_| {
            // The flow target has exactly one home (`TaskGraph::target`), so
            // this parse is the only place an unsupported flow can be caught
            // — it owes the user the list of flows that do work.
            serde::de::Error::custom(format!(
                "unsupported target `{s}`; \
                 supported flows are `xilinx-vitis`, `xilinx-slash`, and `xilinx-hls`"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_forms_round_trip() {
        for (target, wire) in [
            (Target::XilinxVitis, "xilinx-vitis"),
            (Target::XilinxSlash, "xilinx-slash"),
            (Target::XilinxHls, "xilinx-hls"),
        ] {
            assert_eq!(target.as_str(), wire);
            assert_eq!(Target::from_str(wire).expect("parse"), target);
            let json = serde_json::to_string(&target).expect("serialize");
            assert_eq!(json, format!("\"{wire}\""));
            assert_eq!(
                serde_json::from_str::<Target>(&json).expect("deserialize"),
                target,
            );
        }
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
