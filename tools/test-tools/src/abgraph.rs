use serde_json::Value as JsonValue;

use crate::common::{read_json, workspace_path, Result};

#[derive(Debug, PartialEq, Eq)]
struct NormalizedAbgraph {
    vertices: Vec<String>,
    edges: Vec<(i64, i64, String, String)>,
}

pub fn compare_abgraph(name: &str) -> Result<()> {
    let actual = workspace_path(&format!(
        "tests/functional/abgraph/{name}-abgraph-json.json"
    ));
    let golden = workspace_path(&format!("tests/functional/abgraph/golden/{name}.json"));
    let actual = normalize_abgraph(&read_json(&actual)?)?;
    let golden = normalize_abgraph(&read_json(&golden)?)?;
    if actual != golden {
        return Err(format!("{name}: generated ABGraph does not match golden"));
    }
    Ok(())
}

fn normalize_abgraph(graph: &JsonValue) -> Result<NormalizedAbgraph> {
    let vertices = graph
        .get("vs")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| "ABGraph missing array 'vs'".to_string())?
        .iter()
        .map(|vertex| string_field(vertex, "name"))
        .collect::<Result<Vec<_>>>()?;
    let mut vertices = vertices;
    vertices.sort();

    let edges = graph
        .get("es")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| "ABGraph missing array 'es'".to_string())?
        .iter()
        .map(|edge| {
            Ok((
                int_field(edge, "index")?,
                int_field(edge, "width")?,
                string_field(
                    edge.get("source_vertex")
                        .ok_or_else(|| "edge missing source_vertex".to_string())?,
                    "name",
                )?,
                string_field(
                    edge.get("target_vertex")
                        .ok_or_else(|| "edge missing target_vertex".to_string())?,
                    "name",
                )?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut edges = edges;
    edges.sort();

    Ok(NormalizedAbgraph { vertices, edges })
}

fn string_field(value: &JsonValue, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(JsonValue::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("missing string field '{key}'"))
}

fn int_field(value: &JsonValue, key: &str) -> Result<i64> {
    value
        .get(key)
        .and_then(JsonValue::as_i64)
        .ok_or_else(|| format!("missing integer field '{key}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_abgraph_independent_of_order() {
        let first = serde_json::json!({
            "vs": [{"name": "b"}, {"name": "a"}],
            "es": [
                {
                    "index": 2,
                    "width": 32,
                    "source_vertex": {"name": "b"},
                    "target_vertex": {"name": "a"}
                },
                {
                    "index": 1,
                    "width": 64,
                    "source_vertex": {"name": "a"},
                    "target_vertex": {"name": "b"}
                }
            ]
        });
        let second = serde_json::json!({
            "vs": [{"name": "a"}, {"name": "b"}],
            "es": [
                {
                    "index": 1,
                    "width": 64,
                    "source_vertex": {"name": "a"},
                    "target_vertex": {"name": "b"}
                },
                {
                    "index": 2,
                    "width": 32,
                    "source_vertex": {"name": "b"},
                    "target_vertex": {"name": "a"}
                }
            ]
        });
        assert_eq!(
            normalize_abgraph(&first).unwrap(),
            normalize_abgraph(&second).unwrap()
        );
    }
}
