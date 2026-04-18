//! Module definition variants — discriminated by `module_type`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::instantiation::ModuleInstantiation;
use super::support::{ModuleNet, ModuleParameter, ModulePort};

/// Hierarchical name: inner `None` = synthetic, inner `Some(vec![])` = transparent.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct HierarchicalName(pub Option<Vec<String>>);

impl HierarchicalName {
    /// Create a `None` (synthetic) hierarchical name.
    #[must_use]
    pub fn none() -> Self {
        Self(None)
    }

    /// Create a single-element hierarchical name.
    #[must_use]
    pub fn get_name(name: &str) -> Self {
        Self(Some(vec![name.to_owned()]))
    }

    /// Create a hierarchical name from a list of path components.
    #[must_use]
    pub fn from_parts(parts: Vec<String>) -> Self {
        Self(Some(parts))
    }

    /// Returns `true` if this is `None` (synthetic).
    #[must_use]
    pub fn is_none(&self) -> bool {
        self.0.is_none()
    }

    /// Returns the inner name parts, if any.
    #[must_use]
    pub fn as_parts(&self) -> Option<&[String]> {
        self.0.as_deref()
    }
}

/// Base fields shared by all module definitions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BaseFields {
    pub name: String,
    #[serde(default, skip_serializing_if = "HierarchicalName::is_none")]
    pub hierarchical_name: HierarchicalName,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<ModuleParameter>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<ModulePort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, String>>,
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
        extra: BTreeMap<String, Value>,
    },

    #[serde(rename = "verilog_module")]
    Verilog {
        #[serde(flatten)]
        base: BaseFields,
        #[serde(flatten)]
        verilog: VerilogFields,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },

    #[serde(rename = "aux_module")]
    Aux {
        #[serde(flatten)]
        base: BaseFields,
        #[serde(flatten)]
        verilog: VerilogFields,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },

    #[serde(rename = "aux_split_module")]
    AuxSplit {
        #[serde(flatten)]
        base: BaseFields,
        #[serde(flatten)]
        verilog: VerilogFields,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },

    #[serde(rename = "stub_module")]
    Stub {
        #[serde(flatten)]
        base: BaseFields,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },

    #[serde(rename = "pass_through_module")]
    PassThrough {
        #[serde(flatten)]
        base: BaseFields,
        #[serde(flatten)]
        verilog: VerilogFields,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },

    #[serde(rename = "internal_verilog_module")]
    InternalVerilog {
        #[serde(flatten)]
        base: BaseFields,
        #[serde(flatten)]
        verilog: VerilogFields,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
    },

    #[serde(rename = "internal_grouped_module")]
    InternalGrouped {
        #[serde(flatten)]
        base: BaseFields,
        #[serde(flatten)]
        grouped: GroupedFields,
        #[serde(flatten)]
        extra: BTreeMap<String, Value>,
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

    /// Module ports.
    #[must_use]
    pub fn ports(&self) -> &[crate::module::support::ModulePort] {
        match self {
            Self::Grouped { base, .. }
            | Self::Verilog { base, .. }
            | Self::Aux { base, .. }
            | Self::AuxSplit { base, .. }
            | Self::Stub { base, .. }
            | Self::PassThrough { base, .. }
            | Self::InternalVerilog { base, .. }
            | Self::InternalGrouped { base, .. } => &base.ports,
        }
    }

    /// Access the common base fields.
    #[must_use]
    pub fn base(&self) -> &BaseFields {
        match self {
            Self::Grouped { base, .. }
            | Self::Verilog { base, .. }
            | Self::Aux { base, .. }
            | Self::AuxSplit { base, .. }
            | Self::Stub { base, .. }
            | Self::PassThrough { base, .. }
            | Self::InternalVerilog { base, .. }
            | Self::InternalGrouped { base, .. } => base,
        }
    }

    /// Mutable access to the common base fields.
    pub fn base_mut(&mut self) -> &mut BaseFields {
        match self {
            Self::Grouped { base, .. }
            | Self::Verilog { base, .. }
            | Self::Aux { base, .. }
            | Self::AuxSplit { base, .. }
            | Self::Stub { base, .. }
            | Self::PassThrough { base, .. }
            | Self::InternalVerilog { base, .. }
            | Self::InternalGrouped { base, .. } => base,
        }
    }

    /// Build a new grouped module definition.
    #[must_use]
    pub fn new_grouped(
        name: String,
        ports: Vec<super::support::ModulePort>,
        submodules: Vec<super::instantiation::ModuleInstantiation>,
        wires: Vec<super::support::ModuleNet>,
    ) -> Self {
        Self::Grouped {
            base: BaseFields {
                name,
                hierarchical_name: HierarchicalName::none(),
                parameters: Vec::new(),
                ports,
                metadata: None,
            },
            grouped: GroupedFields { submodules, wires },
            extra: BTreeMap::new(),
        }
    }

    /// Build a new Verilog module definition from raw source code.
    #[must_use]
    pub fn new_verilog(name: String, ports: Vec<super::support::ModulePort>, verilog: String) -> Self {
        Self::Verilog {
            base: BaseFields {
                name,
                hierarchical_name: HierarchicalName::none(),
                parameters: Vec::new(),
                ports,
                metadata: None,
            },
            verilog: VerilogFields {
                verilog,
                submodules_module_names: Vec::new(),
            },
            extra: BTreeMap::new(),
        }
    }

    /// Build a new stub module definition.
    #[must_use]
    pub fn new_stub(name: String, ports: Vec<super::support::ModulePort>) -> Self {
        Self::Stub {
            base: BaseFields {
                name,
                hierarchical_name: HierarchicalName::none(),
                parameters: Vec::new(),
                ports,
                metadata: None,
            },
            extra: BTreeMap::new(),
        }
    }

    /// Sort internal collections for deterministic output.
    pub fn normalize(&mut self) {
        match self {
            Self::Grouped { base, grouped, .. }
            | Self::InternalGrouped { base, grouped, .. } => {
                base.ports.sort_unstable_by(|a, b| a.name.cmp(&b.name));
                grouped.submodules.sort_unstable_by(|a, b| a.name.cmp(&b.name));
                grouped.wires.sort_unstable_by(|a, b| a.name.cmp(&b.name));
                for sub in &mut grouped.submodules {
                    sub.connections.sort_unstable_by(|a, b| a.name.cmp(&b.name));
                    sub.parameters.sort_unstable_by(|a, b| a.name.cmp(&b.name));
                }
            }
            Self::Verilog { base, verilog, .. }
            | Self::Aux { base, verilog, .. }
            | Self::AuxSplit { base, verilog, .. }
            | Self::PassThrough { base, verilog, .. }
            | Self::InternalVerilog { base, verilog, .. } => {
                base.ports.sort_unstable_by(|a, b| a.name.cmp(&b.name));
                verilog.submodules_module_names.sort_unstable();
            }
            Self::Stub { base, .. } => {
                base.ports.sort_unstable_by(|a, b| a.name.cmp(&b.name));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hierarchical_name_none() {
        let h = HierarchicalName::none();
        assert!(h.is_none());
        assert!(h.as_parts().is_none());
    }

    #[test]
    fn hierarchical_name_get_name() {
        let h = HierarchicalName::get_name("module_a");
        assert!(!h.is_none());
        assert_eq!(h.as_parts().unwrap(), &["module_a"]);
    }

    #[test]
    fn hierarchical_name_from_parts() {
        let h = HierarchicalName::from_parts(vec!["a".into(), "b".into()]);
        assert_eq!(h.as_parts().unwrap(), &["a", "b"]);
    }

    #[test]
    fn hierarchical_name_default_is_none() {
        let h = HierarchicalName::default();
        assert!(h.is_none());
    }

    #[test]
    fn hierarchical_name_serde_round_trip() {
        let h = HierarchicalName::get_name("signal_a");
        let json = serde_json::to_string(&h).unwrap();
        let h2: HierarchicalName = serde_json::from_str(&json).unwrap();
        assert_eq!(h, h2);
    }

    #[test]
    fn hierarchical_name_none_serde() {
        let h = HierarchicalName::none();
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(json, "null");
        let h2: HierarchicalName = serde_json::from_str(&json).unwrap();
        assert!(h2.is_none());
    }
}
