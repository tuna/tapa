//! `ABGraph` types: vertices, edges, and the placement graph.

use serde::{Deserialize, Serialize};

use crate::area::Area;

/// A vertex in the placement graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ABVertex {
    pub name: String,
    pub sub_cells: Vec<String>,
    pub area: Area,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_slot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserved_slot: Option<String>,
}

/// An edge in the placement graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ABEdge {
    pub source_vertex: String,
    pub target_vertex: String,
    pub index: usize,
    pub width: u64,
}

/// The placement graph for floorplanning.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ABGraph {
    pub vs: Vec<ABVertex>,
    pub es: Vec<ABEdge>,
}

impl ABGraph {
    /// Create an empty graph.
    #[must_use]
    pub fn new() -> Self {
        Self {
            vs: Vec::new(),
            es: Vec::new(),
        }
    }

    /// Add a vertex to the graph.
    pub fn add_vertex(&mut self, vertex: ABVertex) {
        self.vs.push(vertex);
    }

    /// Add an edge to the graph.
    pub fn add_edge(&mut self, edge: ABEdge) {
        self.es.push(edge);
    }

    /// Find a vertex by name.
    #[must_use]
    pub fn find_vertex(&self, name: &str) -> Option<&ABVertex> {
        self.vs.iter().find(|v| v.name == name)
    }
}

impl Default for ABGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abgraph_new_is_empty() {
        let g = ABGraph::new();
        assert!(g.vs.is_empty());
        assert!(g.es.is_empty());
    }

    #[test]
    fn abgraph_add_vertex_and_edge() {
        let mut g = ABGraph::new();
        g.add_vertex(ABVertex {
            name: "producer_0".into(),
            sub_cells: vec!["producer_0".into()],
            area: Area::new(100, 200, 10, 5, 2),
            target_slot: None,
            reserved_slot: None,
        });
        g.add_vertex(ABVertex {
            name: "consumer_0".into(),
            sub_cells: vec!["consumer_0".into()],
            area: Area::new(50, 100, 5, 2, 1),
            target_slot: None,
            reserved_slot: None,
        });
        g.add_edge(ABEdge {
            source_vertex: "producer_0".into(),
            target_vertex: "consumer_0".into(),
            index: 0,
            width: 32,
        });

        assert_eq!(g.vs.len(), 2);
        assert_eq!(g.es.len(), 1);
        assert!(g.find_vertex("producer_0").is_some());
        assert!(g.find_vertex("nonexistent").is_none());
    }

    #[test]
    fn abgraph_serde_round_trip() {
        let mut g = ABGraph::new();
        g.add_vertex(ABVertex {
            name: "v0".into(),
            sub_cells: vec!["v0".into()],
            area: Area::new(10, 20, 3, 1, 0),
            target_slot: Some("SLOT_X0Y0_TO_SLOT_X0Y0".into()),
            reserved_slot: None,
        });
        g.add_edge(ABEdge {
            source_vertex: "v0".into(),
            target_vertex: "v0".into(),
            index: 0,
            width: 64,
        });

        let json = serde_json::to_string(&g).unwrap();
        let g2: ABGraph = serde_json::from_str(&json).unwrap();
        assert_eq!(g, g2);
    }

    #[test]
    fn abgraph_malformed_json_fails() {
        let result = serde_json::from_str::<ABGraph>(r#"{"vs": [{"bad": true}]}"#);
        assert!(
            result.is_err(),
            "malformed JSON should fail"
        );
    }
}
