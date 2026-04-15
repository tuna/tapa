//! Interface type variants — discriminated by `type`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// All 11 interface type variants.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum AnyInterface {
    #[serde(rename = "handshake")]
    HandShake {
        #[serde(default)]
        clk_port: Option<String>,
        #[serde(default)]
        rst_port: Option<String>,
        #[serde(default)]
        ports: Vec<String>,
        #[serde(default)]
        role: String,
        #[serde(default)]
        origin_info: String,
        #[serde(default)]
        ready_port: Option<String>,
        #[serde(default)]
        valid_port: Option<String>,
        #[serde(default)]
        data_ports: Vec<String>,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "feed_forward")]
    FeedForward {
        #[serde(default)]
        clk_port: Option<String>,
        #[serde(default)]
        rst_port: Option<String>,
        #[serde(default)]
        ports: Vec<String>,
        #[serde(default)]
        role: String,
        #[serde(default)]
        origin_info: String,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "false_path")]
    FalsePath {
        #[serde(default)]
        clk_port: Option<String>,
        #[serde(default)]
        rst_port: Option<String>,
        #[serde(default)]
        ports: Vec<String>,
        #[serde(default)]
        role: String,
        #[serde(default)]
        origin_info: String,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "clock")]
    Clock {
        #[serde(default)]
        clk_port: Option<String>,
        #[serde(default)]
        rst_port: Option<String>,
        #[serde(default)]
        ports: Vec<String>,
        #[serde(default)]
        role: String,
        #[serde(default)]
        origin_info: String,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "fp_reset")]
    FalsePathReset {
        #[serde(default)]
        clk_port: Option<String>,
        #[serde(default)]
        rst_port: Option<String>,
        #[serde(default)]
        ports: Vec<String>,
        #[serde(default)]
        role: String,
        #[serde(default)]
        origin_info: String,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "ff_reset")]
    FeedForwardReset {
        #[serde(default)]
        clk_port: Option<String>,
        #[serde(default)]
        rst_port: Option<String>,
        #[serde(default)]
        ports: Vec<String>,
        #[serde(default)]
        role: String,
        #[serde(default)]
        origin_info: String,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "ap_ctrl")]
    ApCtrl {
        #[serde(default)]
        clk_port: Option<String>,
        #[serde(default)]
        rst_port: Option<String>,
        #[serde(default)]
        ports: Vec<String>,
        #[serde(default)]
        role: String,
        #[serde(default)]
        origin_info: String,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "non_pipeline")]
    NonPipeline {
        #[serde(default)]
        clk_port: Option<String>,
        #[serde(default)]
        rst_port: Option<String>,
        #[serde(default)]
        ports: Vec<String>,
        #[serde(default)]
        role: String,
        #[serde(default)]
        origin_info: String,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "unknown")]
    Unknown {
        #[serde(default)]
        clk_port: Option<String>,
        #[serde(default)]
        rst_port: Option<String>,
        #[serde(default)]
        ports: Vec<String>,
        #[serde(default)]
        role: String,
        #[serde(default)]
        origin_info: String,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "tapa_peek")]
    TapaPeek {
        #[serde(default)]
        clk_port: Option<String>,
        #[serde(default)]
        rst_port: Option<String>,
        #[serde(default)]
        ports: Vec<String>,
        #[serde(default)]
        role: String,
        #[serde(default)]
        origin_info: String,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "aux")]
    Aux {
        #[serde(default)]
        clk_port: Option<String>,
        #[serde(default)]
        rst_port: Option<String>,
        #[serde(default)]
        ports: Vec<String>,
        #[serde(default)]
        role: String,
        #[serde(default)]
        origin_info: String,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },
}
