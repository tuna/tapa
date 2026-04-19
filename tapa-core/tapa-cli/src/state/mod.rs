//! Work-directory state bridge — `design.json`, `graph.json`,
//! `settings.json` read / write helpers.

pub mod design;
pub mod graph;
pub mod settings;

pub use design::{load_design, store_design};
pub use graph::{load_graph, store_graph};
pub use settings::{load_settings, store_settings, Settings};

/// Extract a JSON object value as an `IndexMap`. Returns an empty map
/// when the value is absent or not an object. Shared by `build_design`
/// and `floorplan` for `tasks` / `fifos` / area dict projection.
pub fn value_to_indexmap(
    value: Option<&serde_json::Value>,
) -> indexmap::IndexMap<String, serde_json::Value> {
    let Some(serde_json::Value::Object(obj)) = value else {
        return indexmap::IndexMap::new();
    };
    obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}
