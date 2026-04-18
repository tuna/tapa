//! `design.json` serde with annotation preservation and round-trip support.

use crate::error::ParseError;
use crate::program::Program;

/// Top-level design type wrapping Program with parse/serialize methods.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Design {
    pub program: Program,
}

impl Design {
    /// Parse a `design.json` string into a typed Design.
    pub fn from_json(json: &str) -> Result<Self, ParseError> {
        let deserializer = &mut serde_json::Deserializer::from_str(json);
        let program: Program =
            serde_path_to_error::deserialize(deserializer).map_err(|e| {
                ParseError::Json(e.to_string())
            })?;
        Ok(Self { program })
    }

    /// Serialize back to a JSON string.
    pub fn to_json(&self) -> Result<String, ParseError> {
        serde_json::to_string_pretty(&self.program)
            .map_err(|e| ParseError::Json(e.to_string()))
    }

    /// Get the list of slot task names (tasks with `is_slot` = true).
    pub fn floorplan_slots(&self) -> Vec<String> {
        self.program
            .tasks
            .iter()
            .filter(|(_, t)| t.is_slot)
            .map(|(name, _)| name.clone())
            .collect()
    }
}
