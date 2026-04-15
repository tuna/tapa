//! Module definition variants — discriminated by `module_type`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::instantiation::ModuleInstantiation;
use super::support::{ModuleNet, ModuleParameter, ModulePort};

/// Hierarchical name: `None` = synthetic, `Some(vec![])` = transparent.
pub type HierarchicalName = Option<Vec<String>>;

/// Base fields shared by all module definitions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BaseFields {
    pub name: String,
    #[serde(default)]
    pub hierarchical_name: HierarchicalName,
    #[serde(default)]
    pub parameters: Vec<ModuleParameter>,
    #[serde(default)]
    pub ports: Vec<ModulePort>,
    #[serde(default)]
    pub metadata: Option<HashMap<String, String>>,
}

/// Fields specific to grouped modules (submodules + wires).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupedFields {
    #[serde(default)]
    pub submodules: Vec<ModuleInstantiation>,
    #[serde(default)]
    pub wires: Vec<ModuleNet>,
}

/// Fields specific to Verilog modules (raw source code).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerilogFields {
    #[serde(default)]
    pub verilog: String,
    #[serde(default)]
    pub submodules_module_names: Vec<String>,
}

/// All module definition variants, discriminated by `module_type`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "module_type")]
pub enum AnyModuleDefinition {
    #[serde(rename = "grouped_module")]
    Grouped {
        #[serde(flatten)]
        base: BaseFields,
        #[serde(flatten)]
        grouped: GroupedFields,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "verilog_module")]
    Verilog {
        #[serde(flatten)]
        base: BaseFields,
        #[serde(flatten)]
        verilog: VerilogFields,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "aux_module")]
    Aux {
        #[serde(flatten)]
        base: BaseFields,
        #[serde(flatten)]
        verilog: VerilogFields,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "aux_split_module")]
    AuxSplit {
        #[serde(flatten)]
        base: BaseFields,
        #[serde(flatten)]
        verilog: VerilogFields,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "stub_module")]
    Stub {
        #[serde(flatten)]
        base: BaseFields,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "pass_through_module")]
    PassThrough {
        #[serde(flatten)]
        base: BaseFields,
        #[serde(flatten)]
        verilog: VerilogFields,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "internal_verilog_module")]
    InternalVerilog {
        #[serde(flatten)]
        base: BaseFields,
        #[serde(flatten)]
        verilog: VerilogFields,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },

    #[serde(rename = "internal_grouped_module")]
    InternalGrouped {
        #[serde(flatten)]
        base: BaseFields,
        #[serde(flatten)]
        grouped: GroupedFields,
        #[serde(flatten)]
        extra: HashMap<String, Value>,
    },
}

impl AnyModuleDefinition {
    /// Module definition name.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Grouped { base, .. }
            | Self::Verilog { base, .. }
            | Self::Aux { base, .. }
            | Self::AuxSplit { base, .. }
            | Self::Stub { base, .. }
            | Self::PassThrough { base, .. }
            | Self::InternalVerilog { base, .. }
            | Self::InternalGrouped { base, .. } => &base.name,
        }
    }

    /// Sort internal collections for deterministic output.
    pub fn normalize(&mut self) {
        match self {
            Self::Grouped { base, grouped, .. }
            | Self::InternalGrouped { base, grouped, .. } => {
                base.ports.sort_by(|a, b| a.name.cmp(&b.name));
                grouped.submodules.sort_by(|a, b| a.name.cmp(&b.name));
                grouped.wires.sort_by(|a, b| a.name.cmp(&b.name));
                for sub in &mut grouped.submodules {
                    sub.connections.sort_by(|a, b| a.name.cmp(&b.name));
                    sub.parameters.sort_by(|a, b| a.name.cmp(&b.name));
                }
            }
            Self::Verilog { base, .. }
            | Self::Aux { base, .. }
            | Self::AuxSplit { base, .. }
            | Self::Stub { base, .. }
            | Self::PassThrough { base, .. }
            | Self::InternalVerilog { base, .. } => {
                base.ports.sort_by(|a, b| a.name.cmp(&b.name));
            }
        }
    }
}
