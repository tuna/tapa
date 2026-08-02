use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use super::*;
use crate::device::model::{DirCaps, Slot};
use crate::device::select::select_device;
use crate::solver::{LpSolution, VarKind};
use crate::ExactInt;

fn named_terms(model: &LpModel, expr: &LinExpr) -> BTreeMap<String, f64> {
    let mut terms = BTreeMap::new();
    for &(coefficient, var) in &expr.terms {
        let index = usize::try_from(var.0).expect("variable index fits usize");
        let label = model.vars[index].label.clone();
        *terms.entry(label).or_insert(0.0) += coefficient;
    }
    terms
}

fn assert_row<'a>(
    model: &LpModel,
    name: &str,
    op: Comparison,
    rhs: f64,
    expected_terms: impl IntoIterator<Item = (f64, &'a str)>,
) {
    let row = model
        .constraints
        .iter()
        .find(|row| row.name == name)
        .unwrap_or_else(|| panic!("missing model row `{name}`"));
    assert_eq!(row.op, op, "comparison drifted for `{name}`");
    assert_eq!(
        row.rhs.to_bits(),
        rhs.to_bits(),
        "right-hand side drifted for `{name}`"
    );
    assert_eq!(
        named_terms(model, &row.expr),
        expected_terms
            .into_iter()
            .map(|(coefficient, label)| (label.to_string(), coefficient))
            .collect(),
        "coefficients drifted for `{name}`"
    );
}

/// A deterministic test solver that selects a requested region suffix for
/// each x row (or the first candidate), then materializes the *consistent*
/// y assignment and true objective so its incumbent passes full model
/// validation — an inconsistent mock can no longer mask model bugs.
struct ChooseSolver {
    preferred: Mutex<Vec<String>>,
}

impl ChooseSolver {
    fn first() -> Self {
        Self {
            preferred: Mutex::new(Vec::new()),
        }
    }

    fn with_preferences(preferred: Vec<String>) -> Self {
        Self {
            preferred: Mutex::new(preferred),
        }
    }

    /// The chosen candidate per vertex: the first whose label ends with a
    /// preferred suffix, else the first candidate.
    fn selections(model: &LpModel, preferred: &[String]) -> HashMap<String, usize> {
        let mut rows: BTreeMap<String, Vec<&str>> = BTreeMap::new();
        for var in &model.vars {
            let Some(rest) = var.label.strip_prefix("x_") else {
                continue;
            };
            let Some(marker) = rest.find("_SLOT_X") else {
                continue;
            };
            let vertex = rest[..marker].to_string();
            rows.entry(vertex).or_default().push(var.label.as_str());
        }
        rows.into_iter()
            .map(|(vertex, row)| {
                let chosen = row
                    .iter()
                    .position(|label| preferred.iter().any(|suffix| label.ends_with(suffix)))
                    .unwrap_or(0);
                (vertex, chosen)
            })
            .collect()
    }
}

impl Solver for ChooseSolver {
    fn solve(&self, model: &LpModel, _opts: &SolveOpts) -> Result<LpSolution, SolverError> {
        let preferred = self.preferred.lock().expect("lock");
        let chosen = Self::selections(model, &preferred);

        // Chosen x handles and candidate indices per vertex.
        let mut values: HashMap<LpVar, f64> = (0..model.num_vars())
            .map(|index| (LpVar(u32::try_from(index).expect("index")), 0.0))
            .collect();
        let mut vertex_of_x: HashMap<LpVar, String> = HashMap::new();
        let mut x_rows: HashMap<String, Vec<LpVar>> = HashMap::new();
        let mut y_vars: Vec<(usize, usize, usize, LpVar)> = Vec::new();
        for (index, var) in model.vars.iter().enumerate() {
            let handle = LpVar(u32::try_from(index).expect("index"));
            if let Some(rest) = var.label.strip_prefix("x_") {
                if let Some(marker) = rest.find("_SLOT_X") {
                    let vertex = rest[..marker].to_string();
                    vertex_of_x.insert(handle, vertex.clone());
                    x_rows.entry(vertex).or_default().push(handle);
                }
            } else if let Some(rest) = var.label.strip_prefix("y_") {
                let mut indices = rest.split('_').map(str::parse::<usize>);
                let (Some(Ok(ei)), Some(Ok(src)), Some(Ok(dst))) =
                    (indices.next(), indices.next(), indices.next())
                else {
                    panic!("unexpected y label `{}`", var.label);
                };
                y_vars.push((ei, src, dst, handle));
            }
        }
        for (vertex, row) in &x_rows {
            values.insert(row[chosen[vertex]], 1.0);
        }

        // Edge endpoints come from the model's own coupling rows:
        // `edge_{ei}_{side}_{ci}` ties the y plane to one x variable.
        let mut endpoints: HashMap<usize, [Option<String>; 2]> = HashMap::new();
        for constraint in &model.constraints {
            let Some(rest) = constraint.name.strip_prefix("edge_") else {
                continue;
            };
            let mut parts = rest.split('_');
            let ei: usize = parts.next().expect("edge index").parse().expect("index");
            let side = parts.next().expect("src or dst");
            let x_var = constraint
                .expr
                .terms
                .iter()
                .find(|(coefficient, _)| *coefficient < 0.0)
                .map(|(_, var)| *var)
                .expect("a coupling row must reference its x variable");
            // One row per candidate, all naming the same endpoint vertex.
            let slot = &mut endpoints.entry(ei).or_insert([None, None])[usize::from(side == "dst")];
            let vertex = vertex_of_x[&x_var].clone();
            if let Some(existing) = slot {
                debug_assert_eq!(existing, &vertex, "coupling rows name one endpoint");
            } else {
                *slot = Some(vertex);
            }
        }

        // Set the one y per edge consistent with the chosen x pair.
        for (ei, src, dst, handle) in y_vars {
            let [src_vertex, dst_vertex] =
                endpoints.get(&ei).expect("every y edge has coupling rows");
            let consistent = src == chosen[src_vertex.as_ref().expect("src endpoint")]
                && dst == chosen[dst_vertex.as_ref().expect("dst endpoint")];
            if consistent {
                values.insert(handle, 1.0);
            }
        }

        // Report the true objective so the incumbent validates fully.
        let mut objective = model.objective.constant;
        for (coefficient, var) in &model.objective.terms {
            objective += coefficient * values[var];
        }
        let solution = LpSolution {
            status: LpStatus::Optimal,
            objective,
            values,
        };
        if let Err(error) = solution.validate_for(model) {
            panic!(
                "ChooseSolver produced an incumbent that violates its model: {error};                      give the test feasible preferences"
            );
        }
        Ok(solution)
    }
}

struct RecordingSolver {
    inner: ChooseSolver,
    models: Mutex<Vec<LpModel>>,
}

impl RecordingSolver {
    fn first() -> Self {
        Self {
            inner: ChooseSolver::first(),
            models: Mutex::new(Vec::new()),
        }
    }
}

struct RejectFirstRefinementSolver {
    calls: AtomicUsize,
    inner: ChooseSolver,
    models: Mutex<Vec<LpModel>>,
}

impl RejectFirstRefinementSolver {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            inner: ChooseSolver::first(),
            models: Mutex::new(Vec::new()),
        }
    }
}

impl Solver for RejectFirstRefinementSolver {
    fn solve(&self, model: &LpModel, opts: &SolveOpts) -> Result<LpSolution, SolverError> {
        let call = self.calls.fetch_add(1, Ordering::Relaxed);
        self.models.lock().expect("lock").push(model.clone());
        // Calls: row primary, row refinement, parent-atomic primary —
        // reject only the last to force the global atomic retry.
        if call == 2 {
            return Ok(LpSolution {
                status: LpStatus::Infeasible,
                objective: 0.0,
                values: HashMap::new(),
            });
        }
        self.inner.solve(model, opts)
    }
}

impl Solver for RecordingSolver {
    fn solve(&self, model: &LpModel, opts: &SolveOpts) -> Result<LpSolution, SolverError> {
        self.models.lock().expect("lock").push(model.clone());
        self.inner.solve(model, opts)
    }
}

struct StatusSolver {
    status: LpStatus,
    calls: AtomicUsize,
}

impl StatusSolver {
    fn new(status: LpStatus) -> Self {
        Self {
            status,
            calls: AtomicUsize::new(0),
        }
    }
}

impl Solver for StatusSolver {
    fn solve(&self, _model: &LpModel, _opts: &SolveOpts) -> Result<LpSolution, SolverError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(LpSolution {
            status: self.status,
            objective: 0.0,
            values: HashMap::new(),
        })
    }
}

fn vadd_floor_graph() -> FloorGraph {
    let json = r#"{
        "cflags": [], "top": "VecAdd", "target": "xilinx-hls",
        "tasks": {
            "VecAdd": {
                "readable_name": "VecAdd", "code": "void VecAdd() {}", "level": "upper", "synth": "hls",
                "ports": [],
                "tasks": {
                    "A": [{"args": {"out": {"arg": "fifo", "cat": "ostream"}}, "step": 0}],
                    "B": [{"args": {"in": {"arg": "fifo", "cat": "istream"}}, "step": 0}]
                },
                "fifos": {"fifo": {"depth": 2, "consumed_by": ["B", 0], "produced_by": ["A", 0]}}
            },
            "A": {"readable_name": "A", "code": "void A() {}", "level": "lower", "synth": "hls",
                "ports": [{"cat": "ostream", "name": "out", "type": "float", "width": 32}],
                "self_area": {"LUT": 100, "FF": 200}},
            "B": {"readable_name": "B", "code": "void B() {}", "level": "lower", "synth": "hls",
                "ports": [{"cat": "istream", "name": "in", "type": "float", "width": 32}],
                "self_area": {"LUT": 50, "FF": 60}}
        }
    }"#;
    let graph = tapa_ir::TaskGraph::from_json(json).expect("parse");
    let flat = tapa_ir::flatten(&graph).expect("flatten");
    FloorGraph::build(&flat).expect("floor graph")
}

fn parallel_floor_graph(stream_count: usize) -> FloorGraph {
    let mut producer_args = serde_json::Map::new();
    let mut consumer_args = serde_json::Map::new();
    let mut producer_ports = Vec::new();
    let mut consumer_ports = Vec::new();
    let mut fifos = serde_json::Map::new();
    for index in 0..stream_count {
        let port = format!("p{index}");
        let fifo = format!("q{index}");
        producer_args.insert(
            port.clone(),
            serde_json::json!({"arg": fifo.clone(), "cat": "ostream"}),
        );
        consumer_args.insert(
            port.clone(),
            serde_json::json!({"arg": fifo.clone(), "cat": "istream"}),
        );
        producer_ports
            .push(serde_json::json!({"cat": "ostream", "name": port, "type": "int", "width": 32}));
        consumer_ports
            .push(serde_json::json!({"cat": "istream", "name": port, "type": "int", "width": 32}));
        fifos.insert(
            fifo,
            serde_json::json!({
                "depth": 2,
                "produced_by": ["Producer", 0],
                "consumed_by": ["Consumer", 0]
            }),
        );
    }
    let design = serde_json::json!({
        "cflags": [],
        "top": "Top",
        "target": "xilinx-hls",
        "tasks": {
            "Top": {
                "readable_name": "Top", "code": "void Top() {}",
                "level": "upper", "synth": "hls", "ports": [],
                "tasks": {
                    "Producer": [{"args": producer_args, "step": 0}],
                    "Consumer": [{"args": consumer_args, "step": 0}]
                },
                "fifos": fifos
            },
            "Producer": {
                "readable_name": "Producer", "code": "void Producer() {}",
                "level": "lower", "synth": "hls", "ports": producer_ports
            },
            "Consumer": {
                "readable_name": "Consumer", "code": "void Consumer() {}",
                "level": "lower", "synth": "hls", "ports": consumer_ports
            }
        }
    });
    let graph = tapa_ir::TaskGraph::from_json(&design.to_string()).expect("parse");
    let flat = tapa_ir::flatten(&graph).expect("flatten");
    FloorGraph::build(&flat).expect("floor graph")
}

fn single_task_floor_graph(lut: u64) -> FloorGraph {
    single_task_floor_graph_with_area(Area {
        lut,
        ..Area::default()
    })
}

fn single_task_floor_graph_with_area(area: Area) -> FloorGraph {
    let json = serde_json::json!({
        "cflags": [],
        "top": "Top",
        "target": "xilinx-hls",
        "tasks": {
            "Top": {
                "readable_name": "Top",
                "code": "void Top() {}",
                "level": "upper",
                "synth": "hls",
                "ports": [],
                "tasks": {"A": [{"args": {}, "step": 0}]},
                "fifos": {}
            },
            "A": {
                "readable_name": "A",
                "code": "void A() {}",
                "level": "lower",
                "synth": "hls",
                "ports": [],
                "self_area": {
                    "LUT": area.lut,
                    "FF": area.ff,
                    "BRAM_18K": area.bram_18k,
                    "DSP": area.dsp,
                    "URAM": area.uram
                }
            }
        }
    });
    let graph = tapa_ir::TaskGraph::from_json(&json.to_string()).expect("parse");
    let flat = tapa_ir::flatten(&graph).expect("flatten");
    FloorGraph::build(&flat).expect("floor graph")
}

fn one_slot_device(lut: u64) -> Device {
    Device {
        key: "one-slot".to_string(),
        part_num: "xcone".to_string(),
        platform_name: None,
        rows: 1,
        cols: 1,
        is_versal: false,
        user_pblock_name: None,
        slots: vec![Slot {
            x: 0,
            y: 0,
            area: Area {
                lut,
                ..Area::default()
            },
            centroid_x: 0,
            centroid_y: 0,
            pblock_ranges: Vec::new(),
            wire_cap: DirCaps::default(),
            tags: Vec::new(),
        }],
    }
}

fn two_slot_golden_device() -> Device {
    let slot = |y, centroid_y| Slot {
        x: 0,
        y,
        area: Area {
            lut: 1_000,
            ff: 1_000,
            bram_18k: 100,
            dsp: 100,
            uram: 100,
        },
        centroid_x: 0,
        centroid_y,
        pblock_ranges: Vec::new(),
        wire_cap: DirCaps::default(),
        tags: Vec::new(),
    };
    Device {
        key: "two-slot-golden".to_string(),
        part_num: "xctoy".to_string(),
        platform_name: None,
        rows: 2,
        cols: 1,
        is_versal: false,
        user_pblock_name: None,
        slots: vec![slot(0, 0), slot(1, 150)],
    }
}

fn two_slot_golden_model(graph: &FloorGraph) -> FloorplanModel {
    let bottom = Coor::slot(0, 0);
    let top = Coor::slot(0, 1);
    FloorplanModel::build(
        graph,
        &two_slot_golden_device(),
        &vec![vec![bottom, top]; graph.vertices().len()],
        &[Cut {
            name: "y=0".to_string(),
            lhs: vec![bottom],
            rhs: vec![top],
            capacity: 34,
        }],
        DEFAULT_USAGE_LIMIT,
        &PlacementConstraints::default(),
    )
    .expect("golden model")
}

fn mmap_floor_graph() -> FloorGraph {
    let json = r#"{
        "cflags": [], "top": "Top", "target": "xilinx-hls",
        "tasks": {
            "Top": {"readable_name": "Top", "code": "void Top() {}", "level": "upper", "synth": "hls",
                "ports": [{"cat": "mmap", "name": "mem", "type": "ap_uint<512>*", "width": 512}], "tasks": {
                    "R": [{"args": {"m": {"arg": "mem", "cat": "mmap"}}, "step": 0}],
                    "C": [{"args": {}, "step": 0}]}, "fifos": {}},
            "R": {"readable_name": "R", "code": "void R() {}", "level": "lower", "synth": "hls",
                "ports": [{"cat": "mmap", "name": "m", "type": "ap_uint<512>*", "width": 512}],
                "self_area": {"LUT": 400}},
            "C": {"readable_name": "C", "code": "void C() {}", "level": "lower", "synth": "hls",
                "ports": [], "self_area": {"LUT": 400}}
        }
    }"#;
    let graph = tapa_ir::TaskGraph::from_json(json).expect("parse");
    let flat = tapa_ir::flatten(&graph).expect("flatten");
    FloorGraph::build_with_memory(
        &flat,
        &[crate::graph::MemoryInterface {
            endpoint: tapa_ir::AxiEndpoint {
                instance: "R_0".to_string(),
                port: "m".to_string(),
                top_port: "mem".to_string(),
            },
            bank: tapa_ir::MemoryBank {
                kind: tapa_ir::MemoryKind::Hbm,
                index: 7,
            },
            channel_widths: tapa_ir::AxiChannelWidths {
                read_address: 80,
                read_data: 518,
                write_address: 80,
                write_data: 579,
                write_response: 5,
            },
            bridge_instance: None,
        }],
    )
    .expect("floor graph")
}

#[test]
fn canonical_placement_model_matches_expected_formulation() {
    let graph = vadd_floor_graph();
    let bottom = Coor::slot(0, 0);
    let top = Coor::slot(0, 1);
    let model = two_slot_golden_model(&graph);
    let lp = &model.lp;

    let bottom_name = bottom.region_name();
    let top_name = top.region_name();
    let producer_x = [format!("x_A_0_{bottom_name}"), format!("x_A_0_{top_name}")];
    let consumer_x = [format!("x_B_0_{bottom_name}"), format!("x_B_0_{top_name}")];
    let route_y = ["y_0_0_0", "y_0_0_1", "y_0_1_0", "y_0_1_1"];

    assert_eq!(lp.sense, Sense::Minimize);
    assert_eq!(lp.num_vars(), 8, "four sparse x plus four sparse y");
    assert!(lp.vars.iter().all(|var| var.kind == VarKind::Binary
        && var.lower.to_bits() == 0.0_f64.to_bits()
        && var.upper.to_bits() == 1.0_f64.to_bits()));
    assert_eq!(
        lp.vars
            .iter()
            .map(|var| var.label.as_str())
            .collect::<BTreeSet<_>>(),
        producer_x
            .iter()
            .chain(&consumer_x)
            .map(String::as_str)
            .chain(route_y.iter().copied())
            .collect()
    );

    assert_eq!(lp.num_constraints(), 18);
    for (name, vars) in [("vertex_A_0", &producer_x), ("vertex_B_0", &consumer_x)] {
        assert_row(
            lp,
            name,
            Comparison::Eq,
            1.0,
            vars.iter().map(|label| (1.0, label.as_str())),
        );
    }
    assert_row(
        lp,
        "route_0",
        Comparison::Eq,
        1.0,
        route_y.iter().map(|label| (1.0, *label)),
    );
    for (name, terms) in [
        (
            "edge_0_src_0",
            [(1.0, route_y[0]), (1.0, route_y[1]), (-1.0, &producer_x[0])],
        ),
        (
            "edge_0_src_1",
            [(1.0, route_y[2]), (1.0, route_y[3]), (-1.0, &producer_x[1])],
        ),
        (
            "edge_0_dst_0",
            [(1.0, route_y[0]), (1.0, route_y[2]), (-1.0, &consumer_x[0])],
        ),
        (
            "edge_0_dst_1",
            [(1.0, route_y[1]), (1.0, route_y[3]), (-1.0, &consumer_x[1])],
        ),
    ] {
        assert_row(lp, name, Comparison::Eq, 0.0, terms);
    }

    assert_eq!(
        lp.constraints
            .iter()
            .filter(|row| row.name.starts_with("node_"))
            .count(),
        10,
        "five resource rows per active slot"
    );
    for (region, producer, consumer) in [
        (&bottom_name, &producer_x[0], &consumer_x[0]),
        (&top_name, &producer_x[1], &consumer_x[1]),
    ] {
        assert_row(
            lp,
            &format!("node_{region}_LUT_usage"),
            Comparison::Le,
            700.0,
            [(100.0, producer.as_str()), (116.0, consumer.as_str())],
        );
    }
    assert_row(
        lp,
        "cut_y=0_capacity",
        Comparison::Le,
        34.0,
        [(35.0, route_y[1]), (35.0, route_y[2])],
    );

    assert_eq!(lp.objective.constant.to_bits(), 1.0_f64.to_bits());
    assert_eq!(
        named_terms(lp, &lp.objective),
        BTreeMap::from([
            (route_y[1].to_string(), 10_500.0),
            (route_y[2].to_string(), 10_500.0),
        ]),
        "35-bit width times the vertically penalized 150-unit distance"
    );
}

#[test]
fn parallel_streams_share_one_placement_edge_plane() {
    let graph = parallel_floor_graph(2);
    let model = two_slot_golden_model(&graph);
    let route_y = ["y_0_0_0", "y_0_0_1", "y_0_1_0", "y_0_1_1"];

    assert_eq!(graph.streams().len(), 2);
    assert_eq!(graph.placement_edges().len(), 1);
    assert_eq!(graph.placement_edges()[0].width, 70);
    assert_eq!(
        model
            .lp
            .vars
            .iter()
            .filter(|var| var.label.starts_with("y_"))
            .count(),
        4,
        "one endpoint pair allocates one sparse y plane"
    );
    assert_row(
        &model.lp,
        "cut_y=0_capacity",
        Comparison::Le,
        34.0,
        [(70.0, route_y[1]), (70.0, route_y[2])],
    );
    assert_eq!(
        named_terms(&model.lp, &model.lp.objective),
        BTreeMap::from([
            (route_y[1].to_string(), 21_000.0),
            (route_y[2].to_string(), 21_000.0),
        ])
    );
}

#[test]
fn sparse_domains_encode_exact_terminals_and_user_pins() {
    let graph = mmap_floor_graph();
    let device = select_device("u280").expect("u280");
    let regions = atomic_regions(&device);
    let mut constraints = PlacementConstraints::default();
    constraints
        .vertex_regions
        .insert("C_0".to_string(), Coor::slot(1, 2));
    let domains = candidate_domains(
        &graph,
        &device,
        &regions,
        DEFAULT_USAGE_LIMIT,
        None,
        &constraints,
    )
    .expect("domains");

    let reader = graph.index_of("R_0").expect("reader");
    assert_eq!(
        domains[reader], regions,
        "the compute task remains movable; its bank is an ordinary weighted endpoint"
    );
    let terminal = graph.index_of("__tapa_bank_hbm_7").expect("bank terminal");
    assert_eq!(
        domains[terminal],
        [Coor::slot(0, 0)],
        "the exact HBM bank terminal is fixed by its device tag"
    );
    let compute = graph.index_of("C_0").expect("compute");
    assert_eq!(domains[compute], [Coor::slot(1, 2)]);

    let model = FloorplanModel::build(
        &graph,
        &device,
        &domains,
        &find_cuts_for_regions(&device, &regions),
        DEFAULT_USAGE_LIMIT,
        &constraints,
    )
    .expect("model");
    let x_count = model
        .lp
        .vars
        .iter()
        .filter(|var| var.label.starts_with("x_"))
        .count();
    assert_eq!(
        x_count, 8,
        "six reader candidates, one compute pin, and one exact bank terminal"
    );

    let stream_graph = vadd_floor_graph();
    let a = stream_graph.index_of("A_0").expect("A");
    let b = stream_graph.index_of("B_0").expect("B");
    let mut sparse_domains = vec![Vec::new(); stream_graph.vertices().len()];
    sparse_domains[a] = vec![Coor::slot(0, 0), Coor::slot(1, 0)];
    sparse_domains[b] = vec![Coor::slot(0, 0), Coor::slot(1, 0)];
    let sparse_model = FloorplanModel::build(
        &stream_graph,
        &device,
        &sparse_domains,
        &[],
        DEFAULT_USAGE_LIMIT,
        &PlacementConstraints::default(),
    )
    .expect("sparse model");
    let y_count = sparse_model
        .lp
        .vars
        .iter()
        .filter(|var| var.label.starts_with("y_"))
        .count();
    assert_eq!(
        y_count, 4,
        "each edge allocates only src-domain × dst-domain route variables"
    );
}

#[test]
fn multilevel_refinement_preserves_the_selected_parent_row() {
    let graph = vadd_floor_graph();
    let device = select_device("u280").expect("u280");
    let solver = ChooseSolver::with_preferences(vec![
        Coor::span(0, 2, 1, 2).region_name(),
        Coor::slot(1, 2).region_name(),
    ]);
    let result = floorplan_with_strategy(
        &graph,
        &device,
        DEFAULT_USAGE_LIMIT,
        (DEFAULT_USAGE_LIMIT).max(MAX_USAGE_LIMIT),
        PartitionStrategy::MultiLevel,
        &solver,
        &SolveOpts::default(),
    )
    .expect("multilevel floorplan");
    assert!(
        result
            .regions
            .values()
            .all(|region| region == &Coor::slot(1, 2).region_name()),
        "second-pass candidates must remain within the first-pass row"
    );
}

#[test]
fn multilevel_recovers_when_an_aggregate_parent_has_no_feasible_child() {
    let graph = single_task_floor_graph_with_area(Area {
        bram_18k: 75,
        ..Area::default()
    });
    let slot = |x, y, bram_18k| Slot {
        x,
        y,
        area: Area {
            bram_18k,
            ..Area::default()
        },
        centroid_x: i64::from(x) * 100,
        centroid_y: i64::from(y) * 100,
        pblock_ranges: Vec::new(),
        wire_cap: DirCaps::default(),
        tags: Vec::new(),
    };
    let device = Device {
        key: "non-decomposable-row".to_string(),
        part_num: "xctoy".to_string(),
        platform_name: None,
        rows: 2,
        cols: 2,
        is_versal: false,
        user_pblock_name: None,
        slots: vec![
            slot(0, 0, 50),
            slot(1, 0, 50),
            slot(0, 1, 100),
            slot(1, 1, 0),
        ],
    };
    let solver = RecordingSolver::first();

    let result = floorplan_with_strategy(
        &graph,
        &device,
        1.0,
        1.0_f64.max(MAX_USAGE_LIMIT),
        PartitionStrategy::MultiLevel,
        &solver,
        &SolveOpts::default(),
    )
    .expect("the globally feasible atomic placement must survive a bad provisional row");

    assert_eq!(
        result.regions.get("A_0"),
        Some(&Coor::slot(0, 1).region_name())
    );
    assert_eq!(
        solver.models.lock().expect("lock").len(),
        4,
        "the parent-filtered refinement has no domain; the row pass and the              atomic fallback each take a primary solve and its refinement"
    );
}

#[test]
fn multilevel_infeasible_refinement_reuses_the_flat_atomic_formulation() {
    let graph = vadd_floor_graph();
    let device = select_device("u280").expect("u280");
    let logic_limit = 0.7;
    let block_limit = 0.8;
    let solver = RejectFirstRefinementSolver::new();

    floorplan_with_exact_resource_caps(
        &graph,
        &device,
        logic_limit,
        block_limit,
        PartitionStrategy::MultiLevel,
        &solver,
        &SolveOpts::default(),
    )
    .expect("a proven parent-refinement failure must retry atomic placement globally");

    let models = solver.models.lock().expect("lock");
    assert_eq!(
        models.len(),
        5,
        "row primary and refinement, rejected parent-atomic primary,              atomic-fallback primary and refinement"
    );
    let constraints = exact_resource_cap_constraints(&device, block_limit);
    let regions = atomic_regions(&device);
    let domains = candidate_domains(&graph, &device, &regions, logic_limit, None, &constraints)
        .expect("flat domains");
    let expected = FloorplanModel::build(
        &graph,
        &device,
        &domains,
        &find_cuts_for_regions(&device, &regions),
        logic_limit,
        &constraints,
    )
    .expect("flat model");
    assert_eq!(
        crate::solver::write_cplex_lp(&models[3]).expect("render fallback model"),
        crate::solver::write_cplex_lp(&expected.lp).expect("render expected model"),
        "fallback must use the existing atomic variables, rows, and objective unchanged"
    );
}

#[test]
fn readback_rejects_missing_and_fractional_assignments() {
    let graph = vadd_floor_graph();
    let device = select_device("u280").expect("u280");
    let regions = [Coor::slot(0, 0), Coor::slot(1, 0)];
    let domains = vec![regions.to_vec(); graph.vertices().len()];
    let model = FloorplanModel::build(
        &graph,
        &device,
        &domains,
        &[],
        DEFAULT_USAGE_LIMIT,
        &PlacementConstraints::default(),
    )
    .expect("model");

    let missing = LpSolution {
        status: LpStatus::Optimal,
        objective: 0.0,
        values: HashMap::new(),
    };
    assert!(matches!(
        model.read_back(&graph, &domains, &missing),
        Err(IlpError::InvalidSolution(_))
    ));

    let mut values = HashMap::new();
    for row in &model.x {
        values.insert(row[0], 0.5);
        values.insert(row[1], 0.5);
    }
    let fractional = LpSolution {
        status: LpStatus::Optimal,
        objective: 0.0,
        values,
    };
    assert!(matches!(
        model.read_back(&graph, &domains, &fractional),
        Err(IlpError::InvalidSolution(_))
    ));
}

#[test]
fn resource_overrides_use_total_region_capacity() {
    let graph = vadd_floor_graph();
    let device = select_device("u280").expect("u280");
    let region = Coor::slot(0, 0);
    let domains = vec![vec![region]; graph.vertices().len()];
    let mut constraints = PlacementConstraints::default();
    constraints
        .max_resource_limits
        .insert(region.region_name(), BTreeMap::from([(Resource::Lut, 0.5)]));
    let model = FloorplanModel::build(
        &graph,
        &device,
        &domains,
        &[],
        DEFAULT_USAGE_LIMIT,
        &constraints,
    )
    .expect("model");
    let total_lut = device.island_area(&region).expect("area").lut.as_f64();
    let max = model
        .lp
        .constraints
        .iter()
        .find(|constraint| constraint.name == format!("node_{}_LUT_usage", region.region_name()))
        .expect("max");
    assert!(
        (max.rhs - total_lut * 0.5).abs() < f64::EPSILON,
        "slot-specific maximum is based on total, not globally derated, capacity"
    );
}

#[test]
fn exact_multilevel_caps_apply_to_row_and_atomic_resource_rows() {
    let graph = vadd_floor_graph();
    let device = select_device("u280").expect("u280");
    let solver = RecordingSolver::first();
    let logic_limit = 0.7;
    let block_limit = 0.8;

    let (_, strategy) = floorplan_with_exact_resource_caps(
        &graph,
        &device,
        logic_limit,
        block_limit,
        PartitionStrategy::MultiLevel,
        &solver,
        &SolveOpts::default(),
    )
    .expect("exact multilevel floorplan");
    assert_eq!(strategy, PartitionStrategy::MultiLevel);

    let baseline_solver = RecordingSolver::first();
    floorplan_with_strategy(
        &graph,
        &device,
        logic_limit,
        logic_limit,
        PartitionStrategy::MultiLevel,
        &baseline_solver,
        &SolveOpts::default(),
    )
    .expect("single-cap multilevel floorplan");

    let models = solver.models.lock().expect("lock");
    let baseline_models = baseline_solver.models.lock().expect("lock");
    assert_eq!(
        models.len(),
        4,
        "row and atomic iterations, each a primary solve and its refinement"
    );
    assert_eq!(baseline_models.len(), models.len());
    for ((model, baseline), region) in models
        .iter()
        .step_by(2)
        .zip(baseline_models.iter().step_by(2))
        .zip([Coor::span(0, 0, device.cols - 1, 0), Coor::slot(0, 0)])
    {
        assert_eq!(model.num_vars(), baseline.num_vars());
        assert_eq!(model.num_constraints(), baseline.num_constraints());
        assert_eq!(
            model
                .vars
                .iter()
                .map(|variable| variable.label.as_str())
                .collect::<Vec<_>>(),
            baseline
                .vars
                .iter()
                .map(|variable| variable.label.as_str())
                .collect::<Vec<_>>(),
            "the cap policy must not add or remove variables",
        );
        assert_eq!(model.objective, baseline.objective);
        for row in &model.constraints {
            let baseline_row = baseline
                .constraints
                .iter()
                .find(|candidate| candidate.name == row.name)
                .expect("baseline row");
            assert_eq!(row.op, baseline_row.op);
            assert_eq!(row.expr, baseline_row.expr);
        }

        let area = device.island_area(&region).expect("region area");
        for resource in Resource::ALL {
            let row = model
                .constraints
                .iter()
                .find(|row| {
                    row.name == format!("node_{}_{}_usage", region.region_name(), resource.name())
                })
                .expect("resource row");
            let expected = match resource {
                Resource::Ff | Resource::Lut => {
                    scaled_amount(resource.amount(&area), logic_limit).as_f64()
                }
                Resource::Bram18k | Resource::Dsp | Resource::Uram => {
                    resource.amount(&area).as_f64() * block_limit
                }
            };
            assert_eq!(
                row.rhs.to_bits(),
                expected.to_bits(),
                "{} cap drifted for {}",
                resource.name(),
                region.region_name(),
            );
        }
    }
}

#[test]
fn exact_flat_candidates_keep_one_resource_cap() {
    let graph = vadd_floor_graph();
    let device = select_device("u280").expect("u280");
    let solver = RecordingSolver::first();
    let logic_limit = 0.7;

    let (_, strategy) = floorplan_with_exact_resource_caps(
        &graph,
        &device,
        logic_limit,
        0.8,
        PartitionStrategy::Flat,
        &solver,
        &SolveOpts::default(),
    )
    .expect("exact flat floorplan");
    assert_eq!(strategy, PartitionStrategy::Flat);

    let models = solver.models.lock().expect("lock");
    assert_eq!(
        models.len(),
        2,
        "primary solve plus lexicographic refinement"
    );
    let region = Coor::slot(0, 0);
    let area = device.island_area(&region).expect("slot area");
    for resource in Resource::ALL {
        let row = models[0]
            .constraints
            .iter()
            .find(|row| {
                row.name == format!("node_{}_{}_usage", region.region_name(), resource.name())
            })
            .expect("resource row");
        let expected = scaled_amount(resource.amount(&area), logic_limit).as_f64();
        assert_eq!(
            row.rhs.to_bits(),
            expected.to_bits(),
            "flat {} unexpectedly received a block margin",
            resource.name(),
        );
    }
}

#[test]
fn exact_multilevel_candidate_filter_honors_block_overrides() {
    let graph = single_task_floor_graph_with_area(Area {
        bram_18k: 75,
        ..Area::default()
    });
    let mut device = one_slot_device(1_000);
    device.slots[0].area.bram_18k = 100;

    floorplan_with_exact_resource_caps(
        &graph,
        &device,
        0.7,
        0.8,
        PartitionStrategy::MultiLevel,
        &ChooseSolver::first(),
        &SolveOpts::default(),
    )
    .expect("the block margin must keep the task in the candidate domain");

    assert!(matches!(
        floorplan_with_exact_resource_caps(
            &graph,
            &device,
            0.7,
            0.8,
            PartitionStrategy::Flat,
            &ChooseSolver::first(),
            &SolveOpts::default(),
        ),
        Err(IlpError::NoCandidates { .. })
    ));
}

#[test]
fn rectangular_centroid_coefficients_preserve_half_units() {
    let mk_slot = |x, centroid_x| Slot {
        x,
        y: 0,
        area: Area {
            lut: 1000,
            ..Area::default()
        },
        centroid_x,
        centroid_y: 0,
        pblock_ranges: Vec::new(),
        wire_cap: DirCaps::default(),
        tags: Vec::new(),
    };
    let device = Device {
        key: "odd".to_string(),
        part_num: "odd".to_string(),
        platform_name: None,
        rows: 1,
        cols: 3,
        is_versal: false,
        user_pblock_name: None,
        slots: vec![mk_slot(0, 0), mk_slot(1, 1), mk_slot(2, 4)],
    };
    let graph = vadd_floor_graph();
    let domains = vec![vec![Coor::span(0, 0, 1, 0)], vec![Coor::slot(2, 0)]];
    let model = FloorplanModel::build(
        &graph,
        &device,
        &domains,
        &[],
        DEFAULT_USAGE_LIMIT,
        &PlacementConstraints::default(),
    )
    .expect("model");
    assert!(
        model
            .lp
            .objective
            .terms
            .iter()
            .any(|(coefficient, _)| (*coefficient - 122.5).abs() < 1e-9),
        "35-bit physical stream times the exact 3.5-unit centroid distance"
    );
}

#[test]
fn strategy_matches_expected_thresholds() {
    let u280 = select_device("u280").expect("u280");
    assert_eq!(select_strategy(&u280, 299), PartitionStrategy::Flat);
    assert_eq!(select_strategy(&u280, 300), PartitionStrategy::MultiLevel);
    assert_eq!(select_strategy(&u280, 801), PartitionStrategy::MultiLevel);

    let vck = select_device("vck190").expect("vck190");
    assert_eq!(select_strategy(&vck, 300), PartitionStrategy::Flat);
}

#[test]
fn auto_strategy_counts_unique_endpoint_pairs() {
    let graph = parallel_floor_graph(300);
    let device = select_device("u280").expect("u280");

    assert_eq!(graph.streams().len(), 300);
    assert_eq!(graph.placement_edges().len(), 1);
    assert_eq!(
        resolve_strategy(&graph, &device, PartitionStrategy::Auto),
        PartitionStrategy::Flat,
        "parallel FIFOs form one placement edge for schedule selection"
    );
}

#[test]
fn flat_floorplan_assigns_every_vertex_without_cbc() {
    let graph = vadd_floor_graph();
    let device = select_device("u280").expect("u280");
    let result = floorplan_with_strategy(
        &graph,
        &device,
        DEFAULT_USAGE_LIMIT,
        (DEFAULT_USAGE_LIMIT).max(MAX_USAGE_LIMIT),
        PartitionStrategy::Flat,
        &ChooseSolver::first(),
        &SolveOpts::default(),
    )
    .expect("placement");
    assert_eq!(result.regions.len(), graph.vertices().len());
}

#[test]
fn invalid_usage_limit_is_rejected_before_model_building() {
    let graph = vadd_floor_graph();
    let device = select_device("u280").expect("u280");
    assert!(matches!(
        floorplan_with_strategy(
            &graph,
            &device,
            0.0,
            0.0_f64.max(MAX_USAGE_LIMIT),
            PartitionStrategy::Flat,
            &ChooseSolver::first(),
            &SolveOpts::default()
        ),
        Err(IlpError::InvalidLimit { .. })
    ));

    let zero_override = PlacementConfig {
        constraints: PlacementConstraints {
            max_resource_limits: BTreeMap::from([(
                Coor::slot(0, 0).region_name(),
                BTreeMap::from([(Resource::Lut, 0.0)]),
            )]),
            ..PlacementConstraints::default()
        },
        ..PlacementConfig::default()
    };
    validate_config(&zero_override).expect("a zero override can intentionally empty a slot");
}

#[test]
fn area_limited_empty_domain_retries_through_the_usage_ceiling() {
    // The first case becomes legal on the regular 0.72 step. The second
    // remains illegal at 0.94 and verifies that the exact 0.95 ceiling is
    // attempted instead of being skipped by a 0.02 increment.
    for lut in [719, 949] {
        let graph = single_task_floor_graph(lut);
        let result = floorplan_with_strategy(
            &graph,
            &one_slot_device(1000),
            DEFAULT_USAGE_LIMIT,
            (DEFAULT_USAGE_LIMIT).max(MAX_USAGE_LIMIT),
            PartitionStrategy::Flat,
            &ChooseSolver::first(),
            &SolveOpts::default(),
        )
        .expect("area-only domain failure should retry");
        assert_eq!(
            result.regions.get("A_0"),
            Some(&Coor::slot(0, 0).region_name())
        );
    }
}

#[test]
fn exact_usage_limit_disables_infeasibility_retries() {
    let graph = vadd_floor_graph();
    let device = select_device("u280").expect("u280");

    let ordinary_solver = StatusSolver::new(LpStatus::Infeasible);
    assert!(matches!(
        floorplan_with_strategy(&graph, &device, DEFAULT_USAGE_LIMIT, (DEFAULT_USAGE_LIMIT).max(MAX_USAGE_LIMIT), PartitionStrategy::Flat, &ordinary_solver, &SolveOpts::default()),
        Err(IlpError::Infeasible(limit)) if limit == MAX_USAGE_LIMIT
    ));
    assert!(
        ordinary_solver.calls.load(Ordering::Relaxed) > 1,
        "ordinary floorplanning must retain utilization retries",
    );

    let exact_solver = StatusSolver::new(LpStatus::Infeasible);
    assert!(matches!(
        floorplan_with_strategy(
            &graph,
            &device,
            DEFAULT_USAGE_LIMIT,
            DEFAULT_USAGE_LIMIT,
            PartitionStrategy::Flat,
            &exact_solver,
            &SolveOpts::default(),
        ),
        Err(IlpError::Infeasible(limit)) if limit == DEFAULT_USAGE_LIMIT
    ));
    assert_eq!(
        exact_solver.calls.load(Ordering::Relaxed),
        1,
        "an exact DSE candidate must perform only its requested solve",
    );
}

#[test]
fn zero_area_vertex_fits_a_zero_derated_slot() {
    // A radically small usage limit floors every scaled slot resource to
    // zero; a resource-free vertex still fits (0 <= 0) and must not be
    // rejected as candidate-less.
    let graph = single_task_floor_graph(0);
    let device = one_slot_device(1000);
    let assignment = floorplan_with_strategy(
        &graph,
        &device,
        1e-12,
        1e-12,
        PartitionStrategy::Flat,
        &ChooseSolver::first(),
        &SolveOpts::default(),
    )
    .expect("the fitting check alone decides a zero-area vertex");
    assert_eq!(assignment.regions.len(), 1);
}

#[test]
fn refinement_solve_pins_the_primary_objective_and_ranks_candidates() {
    let graph = single_task_floor_graph(1);
    let device = one_slot_device(1000);
    let solver = RecordingSolver::first();
    floorplan_with_strategy(
        &graph,
        &device,
        1.0,
        1.0,
        PartitionStrategy::Flat,
        &solver,
        &SolveOpts::default(),
    )
    .expect("floorplan");

    let models = solver.models.lock().expect("lock");
    assert_eq!(
        models.len(),
        2,
        "primary solve plus lexicographic refinement"
    );
    let refined = &models[1];
    let pin = refined
        .constraints
        .iter()
        .find(|constraint| constraint.name == "lexicographic_pin")
        .expect("the primary objective must be pinned");
    assert_eq!(pin.op, Comparison::Le);
    assert_eq!(
        pin.rhs.to_bits(),
        1.0_f64.to_bits(),
        "the edge-less primary objective is its constant 1.0",
    );
    assert_eq!(
        pin.expr.constant.to_bits(),
        1.0_f64.to_bits(),
        "the pin row carries the primary objective's constant",
    );
    // The single candidate has rank 0, so the secondary objective is empty.
    assert!(refined.objective.terms.is_empty());
}

#[test]
fn permanent_pin_conflict_does_not_retry_or_solve() {
    let graph = single_task_floor_graph(1);
    let device = one_slot_device(1000);
    let solver = StatusSolver::new(LpStatus::Infeasible);
    let config = PlacementConfig {
        strategy: PartitionStrategy::Flat,
        constraints: PlacementConstraints {
            vertex_regions: BTreeMap::from([("A_0".to_string(), Coor::slot(1, 0))]),
            ..PlacementConstraints::default()
        },
        ..PlacementConfig::default()
    };

    assert!(matches!(
        floorplan_with_config(&graph, &device, &config, &solver, &SolveOpts::default()),
        Err(IlpError::NoCandidates { vertex }) if vertex == "A_0"
    ));
    assert_eq!(
        solver.calls.load(Ordering::Relaxed),
        0,
        "a utilization increase cannot repair a permanent pin conflict"
    );
}

#[test]
fn unsolved_status_is_not_disguised_as_a_utilization_retry() {
    let graph = vadd_floor_graph();
    let device = select_device("u280").expect("u280");
    let solver = StatusSolver::new(LpStatus::NotSolved);
    assert!(matches!(
        floorplan_with_strategy(
            &graph,
            &device,
            DEFAULT_USAGE_LIMIT,
            (DEFAULT_USAGE_LIMIT).max(MAX_USAGE_LIMIT),
            PartitionStrategy::Flat,
            &solver,
            &SolveOpts::default()
        ),
        Err(IlpError::NoIncumbent(LpStatus::NotSolved))
    ));
    assert_eq!(
        solver.calls.load(Ordering::Relaxed),
        1,
        "only proven infeasibility may trigger a higher-utilization solve"
    );
}
