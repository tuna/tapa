//! Interface type variants — discriminated by `type`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Common base fields shared by all interface types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InterfaceBase {
    #[serde(default)]
    pub clk_port: Option<String>,
    #[serde(default)]
    pub rst_port: Option<String>,
    #[serde(default)]
    pub ports: Vec<String>,
    #[serde(default)]
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
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "feed_forward")]
    FeedForward {
        #[serde(flatten)]
        base: InterfaceBase,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "false_path")]
    FalsePath {
        #[serde(flatten)]
        base: InterfaceBase,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "clock")]
    Clock {
        #[serde(flatten)]
        base: InterfaceBase,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "fp_reset")]
    FalsePathReset {
        #[serde(flatten)]
        base: InterfaceBase,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "ff_reset")]
    FeedForwardReset {
        #[serde(flatten)]
        base: InterfaceBase,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
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
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "non_pipeline")]
    NonPipeline {
        #[serde(flatten)]
        base: InterfaceBase,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "unknown")]
    Unknown {
        #[serde(flatten)]
        base: InterfaceBase,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "tapa_peek")]
    TapaPeek {
        #[serde(flatten)]
        base: InterfaceBase,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "aux")]
    Aux {
        #[serde(flatten)]
        base: InterfaceBase,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },
}
