//! Root project container.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::blackbox::BlackBox;
use crate::error::ParseError;
use crate::module::definition::AnyModuleDefinition;

/// Collection of module definitions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Modules {
    /// Namespace name (typically `"$root"`).
    pub name: String,
    /// Module definitions sorted by name.
    pub module_definitions: Vec<AnyModuleDefinition>,
    /// Name of the top-level module.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_name: Option<String>,
}

/// Root of the `GraphIR` JSON (`graphir.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Project {
    /// FPGA part number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_num: Option<String>,
    /// Module definitions.
    pub modules: Modules,
    /// External blackbox files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blackboxes: Vec<BlackBox>,
    /// Interface definitions per module.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ifaces: Option<BTreeMap<String, Vec<crate::interface::AnyInterface>>>,
    /// RS pragma annotations per module.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_to_rtl_pragmas: Option<BTreeMap<String, Vec<String>>>,
    /// Old-style RS pragmas (nested dicts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_to_old_rtl_pragmas: Option<BTreeMap<String, Value>>,
    /// Floorplan island → pblock range mapping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub island_to_pblock_range: Option<BTreeMap<String, Vec<String>>>,
    /// Route paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routes: Option<Vec<Value>>,
    /// Resource usage upper bounds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_to_max_local_usage: Option<BTreeMap<String, f64>>,
    /// Cut crossing counts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut_to_crossing_count: Option<BTreeMap<String, f64>>,
    /// Forward-compatibility: preserve any unknown top-level fields.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Project {
    /// Parse a `graphir.json` payload with field-path error diagnostics.
    ///
    /// Runs post-deserialize validation including `BlackBox` payload checks.
    pub fn from_json(json: &str) -> Result<Self, ParseError> {
        let de = &mut serde_json::Deserializer::from_str(json);
        let project: Self =
            serde_path_to_error::deserialize(de).map_err(|e| ParseError::Schema {
                path: e.path().to_string(),
                message: e.inner().to_string(),
            })?;
        project.validate_blackboxes()?;
        project.validate_port_types()?;
        project.validate_interfaces()?;
        Ok(project)
    }

    /// Sort all collections for deterministic serialization output.
    /// Mirrors Python's `NamespaceModel` sort behavior.
    pub fn normalize(&mut self) {
        self.modules.module_definitions.sort_unstable_by(|a, b| a.name().cmp(b.name()));
        for def in &mut self.modules.module_definitions {
            def.normalize();
        }
    }

    /// Serialize to JSON string (with deterministic ordering).
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        let mut normalized = self.clone();
        normalized.normalize();
        serde_json::to_string_pretty(&normalized)
    }

    /// Serialize to JSON string without normalization.
    pub fn to_json_raw(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Validate that module port types are valid Verilog directions.
    fn validate_port_types(&self) -> Result<(), ParseError> {
        const VALID_PORT_TYPES: &[&str] = &[
            "input wire",
            "output wire",
            "output reg",
            "inout wire",
            "input",
            "output",
            "inout",
        ];
        for def in &self.modules.module_definitions {
            let ports = def.ports();
            for port in ports {
                if !VALID_PORT_TYPES.contains(&port.port_type.as_str()) {
                    return Err(ParseError::Schema {
                        path: format!("modules.{}.port.{}", def.name(), port.name),
                        message: format!(
                            "invalid port type `{}`; expected one of: {}",
                            port.port_type,
                            VALID_PORT_TYPES.join(", ")
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    /// Validate that all interfaces have at least one port.
    fn validate_interfaces(&self) -> Result<(), ParseError> {
        if let Some(ifaces) = &self.ifaces {
            for (module, iface_list) in ifaces {
                for (i, iface) in iface_list.iter().enumerate() {
                    let base = iface.base();
                    if base.ports.is_empty() {
                        return Err(ParseError::Schema {
                            path: format!("ifaces.{module}[{i}]"),
                            message: format!(
                                "{} interface must have at least one port",
                                iface.type_name()
                            ),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// Look up a module definition by name.
    #[must_use]
    pub fn get_module(&self, name: &str) -> Option<&AnyModuleDefinition> {
        self.modules
            .module_definitions
            .iter()
            .find(|m| m.name() == name)
    }

    /// Look up the top-level module definition.
    #[must_use]
    pub fn get_top_module(&self) -> Option<&AnyModuleDefinition> {
        self.modules
            .top_name
            .as_ref()
            .and_then(|name| self.get_module(name))
    }

    /// Check whether a module definition with the given name exists.
    #[must_use]
    pub fn has_module(&self, name: &str) -> bool {
        self.modules
            .module_definitions
            .iter()
            .any(|m| m.name() == name)
    }

    /// Validate all blackbox payloads (base64 decode + zlib decompress).
    pub fn validate_blackboxes(&self) -> Result<(), ParseError> {
        for (i, bb) in self.blackboxes.iter().enumerate() {
            bb.get_binary().map_err(|e| ParseError::Schema {
                path: format!("blackboxes[{i}].base64"),
                message: e.to_string(),
            })?;
        }
        Ok(())
    }
}
