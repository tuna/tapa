//! Interface type variants — discriminated by `type`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

fn default_role() -> String {
    "to_be_determined".to_owned()
}

/// Common base fields shared by all interface types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InterfaceBase {
    #[serde(default)]
    pub clk_port: Option<String>,
    #[serde(default)]
    pub rst_port: Option<String>,
    #[serde(default)]
    pub ports: Vec<String>,
    #[serde(default = "default_role")]
    pub role: String,
    #[serde(default)]
    pub origin_info: String,
}

/// All 11 interface type variants.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum AnyInterface {
    #[serde(rename = "handshake")]
    HandShake {
        #[serde(flatten)]
        base: InterfaceBase,
        ready_port: Option<String>,
        valid_port: Option<String>,
        #[serde(default)]
        data_ports: Vec<String>,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },

    #[serde(rename = "feed_forward")]
    FeedForward {
        #[serde(flatten)]
        base: InterfaceBase,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },

    #[serde(rename = "false_path")]
    FalsePath {
        #[serde(flatten)]
        base: InterfaceBase,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },

    #[serde(rename = "clock")]
    Clock {
        #[serde(flatten)]
        base: InterfaceBase,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },

    #[serde(rename = "fp_reset")]
    FalsePathReset {
        #[serde(flatten)]
        base: InterfaceBase,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },

    #[serde(rename = "ff_reset")]
    FeedForwardReset {
        #[serde(flatten)]
        base: InterfaceBase,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },

    #[serde(rename = "ap_ctrl")]
    ApCtrl {
        #[serde(flatten)]
        base: InterfaceBase,
        #[serde(default)]
        ap_start_port: Option<String>,
        #[serde(default)]
        ap_ready_port: Option<String>,
        #[serde(default)]
        ap_done_port: Option<String>,
        #[serde(default)]
        ap_idle_port: Option<String>,
        #[serde(default)]
        ap_continue_port: Option<String>,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },

    #[serde(rename = "non_pipeline")]
    NonPipeline {
        #[serde(flatten)]
        base: InterfaceBase,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },

    #[serde(rename = "unknown")]
    Unknown {
        #[serde(flatten)]
        base: InterfaceBase,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },

    #[serde(rename = "tapa_peek")]
    TapaPeek {
        #[serde(flatten)]
        base: InterfaceBase,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },

    #[serde(rename = "aux")]
    Aux {
        #[serde(flatten)]
        base: InterfaceBase,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },
}

impl AnyInterface {
    /// Access the common base fields.
    #[must_use]
    pub fn base(&self) -> &InterfaceBase {
        match self {
            Self::HandShake { base, .. }
            | Self::FeedForward { base, .. }
            | Self::FalsePath { base, .. }
            | Self::Clock { base, .. }
            | Self::FalsePathReset { base, .. }
            | Self::FeedForwardReset { base, .. }
            | Self::ApCtrl { base, .. }
            | Self::NonPipeline { base, .. }
            | Self::Unknown { base, .. }
            | Self::TapaPeek { base, .. }
            | Self::Aux { base, .. } => base,
        }
    }

    /// Mutable access to the common base fields.
    #[must_use]
    pub fn base_mut(&mut self) -> &mut InterfaceBase {
        match self {
            Self::HandShake { base, .. }
            | Self::FeedForward { base, .. }
            | Self::FalsePath { base, .. }
            | Self::Clock { base, .. }
            | Self::FalsePathReset { base, .. }
            | Self::FeedForwardReset { base, .. }
            | Self::ApCtrl { base, .. }
            | Self::NonPipeline { base, .. }
            | Self::Unknown { base, .. }
            | Self::TapaPeek { base, .. }
            | Self::Aux { base, .. } => base,
        }
    }

    /// Interface type name for diagnostics.
    #[must_use]
    pub fn type_name(&self) -> &str {
        match self {
            Self::HandShake { .. } => "handshake",
            Self::FeedForward { .. } => "feed_forward",
            Self::FalsePath { .. } => "false_path",
            Self::Clock { .. } => "clock",
            Self::FalsePathReset { .. } => "fp_reset",
            Self::FeedForwardReset { .. } => "ff_reset",
            Self::ApCtrl { .. } => "ap_ctrl",
            Self::NonPipeline { .. } => "non_pipeline",
            Self::Unknown { .. } => "unknown",
            Self::TapaPeek { .. } => "tapa_peek",
            Self::Aux { .. } => "aux",
        }
    }

    /// Returns the list of data-flow ports (excluding clk/rst).
    #[must_use]
    pub fn data_ports(&self) -> Vec<String> {
        let base = self.base();
        let clk = base.clk_port.as_deref();
        let rst = base.rst_port.as_deref();
        base.ports
            .iter()
            .filter(|p| Some(p.as_str()) != clk && Some(p.as_str()) != rst)
            .cloned()
            .collect()
    }
}
