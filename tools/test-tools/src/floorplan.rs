use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::common::Result;

const SLOT_FORMAT_PREFIX: &str = "SLOT_X";
const FLOORPLAN_SEED0_SLOTS: &[(u64, u64)] = &[
    (3, 3),
    (0, 2),
    (3, 3),
    (2, 3),
    (2, 1),
    (1, 2),
    (1, 0),
    (2, 1),
    (2, 0),
    (0, 2),
    (3, 0),
    (2, 3),
    (2, 1),
    (3, 3),
    (2, 0),
    (0, 0),
    (3, 0),
];

const FLOORPLAN_LEAVES: &[(&str, &[&str])] = &[
    (
        "bandwidth",
        &[
            "Bandwidth_fsm",
            "Copy_0",
            "Copy_1",
            "Copy_2",
            "Copy_3",
            "chan_0",
            "chan_1",
            "chan_2",
            "chan_3",
        ],
    ),
    (
        "cannon",
        &[
            "Gather_0",
            "ProcElem_0",
            "ProcElem_1",
            "ProcElem_2",
            "ProcElem_3",
            "Scatter_0",
            "Scatter_1",
            "a_vec",
            "b_vec",
            "b_vec",
        ],
    ),
    (
        "gemv",
        &["GemvCore_0", "GemvCore_1", "mat_a", "vec_x", "vec_y"],
    ),
    (
        "graph",
        &[
            "Control_0",
            "Graph_fsm",
            "ProcElem_0",
            "UpdateHandler_0",
            "edges",
            "num_edges",
            "num_vertices",
            "updates",
            "vertices",
        ],
    ),
    (
        "jacobi",
        &[
            "Mmap2Stream",
            "Module0Func",
            "Module1Func#1",
            "Module1Func#2",
            "Module1Func#3",
            "Module1Func#4",
            "Module2Func#1",
            "Module2Func#2",
            "Module3Func#1",
            "Module3Func#2",
            "Module6Func#1",
            "Module6Func#2",
            "Module8Func",
            "Stream2Mmap",
        ],
    ),
    (
        "network",
        &[
            "Consume_0",
            "Network_fsm",
            "Produce_0",
            "Switch2x2_0",
            "Switch2x2_1",
            "Switch2x2_10",
            "Switch2x2_11",
            "Switch2x2_2",
            "Switch2x2_3",
            "Switch2x2_4",
            "Switch2x2_5",
            "Switch2x2_6",
            "Switch2x2_7",
            "Switch2x2_8",
            "Switch2x2_9",
            "mmap_in",
            "mmap_out",
        ],
    ),
    (
        "shared_mmap",
        &[
            "Add_0",
            "Mmap2Stream_0",
            "Mmap2Stream_1",
            "Stream2Mmap_0",
            "VecAddShared_fsm",
            "elems",
        ],
    ),
    (
        "vadd",
        &[
            "Add_0",
            "Mmap2Stream_0",
            "Mmap2Stream_1",
            "Stream2Mmap_0",
            "VecAdd_fsm",
            "a",
            "b",
            "c",
        ],
    ),
];

pub fn gen_floorplan(index: u64, app: &str, output: &Path) -> Result<()> {
    let leaves = FLOORPLAN_LEAVES
        .iter()
        .find_map(|(name, leaves)| (*name == app).then_some(*leaves))
        .ok_or_else(|| format!("unknown floorplan app '{app}'"))?;
    let floorplan = leaves
        .iter()
        .enumerate()
        .map(|(i, leaf)| {
            let (x, y) = floorplan_slot(index, i);
            (
                (*leaf).to_string(),
                format!("{SLOT_FORMAT_PREFIX}{x}Y{y}:{SLOT_FORMAT_PREFIX}{x}Y{y}"),
            )
        })
        .collect::<BTreeMap<_, _>>();
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    let data = serde_json::to_string_pretty(&floorplan)
        .map_err(|error| format!("failed to encode floorplan: {error}"))?;
    fs::write(output, format!("{data}\n"))
        .map_err(|error| format!("failed to write {}: {error}", output.display()))
}

fn floorplan_slot(index: u64, position: usize) -> (u64, u64) {
    if index == 0 && position < FLOORPLAN_SEED0_SLOTS.len() {
        return FLOORPLAN_SEED0_SLOTS[position];
    }
    let mut rng = SplitMix64::new(index ^ (position as u64).wrapping_mul(0x517c_c1b7_2722_0a95));
    (rng.next_range(4), rng.next_range(4))
}

struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    fn next_range(&mut self, limit: u64) -> u64 {
        self.next() % limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floorplan_generation_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.json");
        let second = dir.path().join("second.json");
        gen_floorplan(7, "vadd", &first).unwrap();
        gen_floorplan(7, "vadd", &second).unwrap();
        assert_eq!(
            fs::read_to_string(first).unwrap(),
            fs::read_to_string(second).unwrap()
        );
    }
}
