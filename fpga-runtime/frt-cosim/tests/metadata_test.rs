use std::collections::BTreeMap;

use frt_cosim::metadata::{self, ArgKind, Mode, StreamDir, StreamProtocol};
use indexmap::IndexMap;
use tapa_ir::port::{ArgCategory, Port};
use tapa_ir::{SynthTarget, Target, Task, TaskGraph, TaskLevel};

const KERNEL_XML: &str = r#"<?xml version="1.0"?>
<root>
  <kernel name="vadd">
    <args>
      <arg name="a" addressQualifier="1" id="0" port="m_axi_a" dataWidth="512" addrWidth="64"/>
      <arg name="n" addressQualifier="0" id="1" width="32"/>
      <arg name="s" addressQualifier="4" id="2" port="s_axis_s" dataWidth="32" depth="16"/>
    </args>
  </kernel>
</root>"#;

#[test]
fn parse_vitis_kernel_xml() {
    let spec =
        metadata::xo::parse_kernel_xml(KERNEL_XML, std::path::Path::new("/tmp")).expect("parse");
    assert_eq!(spec.top_name, "vadd");
    assert_eq!(spec.mode, Mode::Vitis);
    assert_eq!(spec.args.len(), 3);
    assert!(matches!(spec.args[0].kind, ArgKind::Mmap { .. }));
    assert!(matches!(spec.args[1].kind, ArgKind::Scalar { .. }));
    assert!(matches!(
        spec.args[2].kind,
        ArgKind::Stream {
            protocol: StreamProtocol::Axis,
            ..
        }
    ));
}

/// A task graph shaped like the one `tapa pack` puts in the archive's
/// `tapa.json`: `cat`-tagged ports on the top task, with plural categories
/// fanning out per channel.
fn task_graph(ports: Vec<Port>) -> TaskGraph {
    let mut tasks = BTreeMap::new();
    tasks.insert(
        "vadd".to_owned(),
        Task {
            level: TaskLevel::Lower,
            code: "void vadd() {}".to_owned(),
            readable_name: "vadd".to_owned(),
            synth: SynthTarget::Hls,
            ports,
            tasks: BTreeMap::new(),
            fifos: BTreeMap::new(),
            clock_period: String::new(),
            self_area: IndexMap::new(),
            total_area: IndexMap::new(),
        },
    );
    TaskGraph {
        top: "vadd".to_owned(),
        target: Target::XilinxHls,
        cflags: vec![],
        tasks,
    }
}

fn port(name: &str, cat: ArgCategory, width: u32, chan_count: Option<u32>) -> Port {
    Port {
        cat,
        name: name.to_owned(),
        ctype: "int".to_owned(),
        width,
        chan_count,
        chan_size: None,
    }
}

#[test]
fn project_hls_task_graph() {
    let graph = task_graph(vec![
        port("a", ArgCategory::Mmap, 32, None),
        port("s", ArgCategory::Istream, 32, None),
        port("out", ArgCategory::Ostreams, 32, Some(2)),
    ]);
    let spec = metadata::zip_pkg::spec_from_task_graph(&graph).expect("project");
    assert_eq!(spec.top_name, "vadd");
    assert_eq!(spec.mode, Mode::Hls);
    assert_eq!(spec.args[0].name, "a");
    assert_eq!(spec.args[1].name, "s_s");
    assert_eq!(spec.args[2].name, "out_0");
    assert_eq!(spec.args[3].name, "out_1");
    assert!(matches!(
        &spec.args[1].kind,
        ArgKind::Stream {
            dir: StreamDir::In,
            protocol: StreamProtocol::ApFifo,
            ..
        }
    ));
}

/// Argument ids are the kernel ABI the simulator binds against: they must be
/// a dense sequence in port declaration order, with plural categories
/// consuming one id per channel.
#[test]
fn arg_ids_are_dense_and_in_declaration_order() {
    let graph = task_graph(vec![
        port("a", ArgCategory::Mmap, 32, None),
        port("s", ArgCategory::Istream, 32, None),
        port("out", ArgCategory::Ostreams, 32, Some(2)),
        port("n", ArgCategory::Scalar, 32, None),
    ]);
    let spec = metadata::zip_pkg::spec_from_task_graph(&graph).expect("project");
    let ids: Vec<u32> = spec.args.iter().map(|a| a.id).collect();
    assert_eq!(ids, vec![0, 1, 2, 3, 4], "ids must be dense and ordered");
    assert_eq!(spec.args[4].name, "n", "the scalar keeps its own name");
}

/// The schema carries no per-port `depth` / `addr_width`, so every stream and
/// mmap gets the fixed value the cosim harness assumes. Pinned because these
/// are hardcoded on this side of the boundary now.
#[test]
fn stream_depth_and_mmap_addr_width_are_the_fixed_defaults() {
    let graph = task_graph(vec![
        port("a", ArgCategory::Mmap, 512, None),
        port("s", ArgCategory::Istream, 32, None),
    ]);
    let spec = metadata::zip_pkg::spec_from_task_graph(&graph).expect("project");
    assert_eq!(
        spec.args[0].kind,
        ArgKind::Mmap {
            data_width: 512,
            addr_width: 64,
        },
    );
    assert_eq!(
        spec.args[1].kind,
        ArgKind::Stream {
            width: 32,
            depth: 16,
            dir: StreamDir::In,
            protocol: StreamProtocol::ApFifo,
        },
    );
}

/// `async_mmap` binds exactly like `mmap`; `immap` / `ommap` have never been
/// wired up and must stay a loud error rather than silently binding as one.
#[test]
fn mmap_categories_are_bound_or_rejected_explicitly() {
    let graph = task_graph(vec![port("a", ArgCategory::AsyncMmap, 32, None)]);
    let spec = metadata::zip_pkg::spec_from_task_graph(&graph).expect("async_mmap projects");
    assert!(matches!(spec.args[0].kind, ArgKind::Mmap { .. }));

    for cat in [ArgCategory::Immap, ArgCategory::Ommap] {
        let graph = task_graph(vec![port("a", cat, 32, None)]);
        let err = metadata::zip_pkg::spec_from_task_graph(&graph)
            .expect_err("immap/ommap must be rejected");
        assert!(
            err.to_string().contains(cat.as_str()),
            "error must name the unsupported category; got {err}",
        );
    }
}

/// An `hmap<T, N, S>` is a single host buffer that the host splits into `N`
/// kernel `m_axi` arguments (`{name}_{i}`), matching the `kernel.xml` ports
/// `tapa pack` projects and the crossbar masters `tapa-codegen` emits. The
/// frontend sets `chan_count` for `hmap` and nothing else, so it -- not the
/// category, which normalizes to `mmap` on the wire -- is what marks the
/// fan-out. Binding one argument would bind the wrong ports and silently
/// shift every later argument's id.
#[test]
fn hmap_port_fans_out_one_arg_per_channel() {
    let graph = task_graph(vec![
        port("mat_a", ArgCategory::Mmap, 512, Some(2)),
        port("vec_x", ArgCategory::Mmap, 512, None),
        port("n", ArgCategory::Scalar, 32, None),
    ]);
    let spec = metadata::zip_pkg::spec_from_task_graph(&graph).expect("project");
    let names: Vec<&str> = spec.args.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["mat_a_0", "mat_a_1", "vec_x", "n"],
        "each hmap channel binds its own m_axi arg; a plain mmap keeps its name",
    );
    let ids: Vec<u32> = spec.args.iter().map(|a| a.id).collect();
    assert_eq!(ids, vec![0, 1, 2, 3], "ids stay dense across the fan-out");
    for arg in &spec.args[..2] {
        assert_eq!(
            arg.kind,
            ArgKind::Mmap {
                data_width: 512,
                addr_width: 64,
            },
            "every channel carries the port's own width",
        );
    }
}

/// `chan_count = Some(1)` is still an hmap -- it gets the crossbar in
/// `tapa-codegen` and an indexed `kernel.xml` port -- so it binds `mat_a_0`,
/// not `mat_a`.
#[test]
fn single_channel_hmap_still_binds_an_indexed_arg() {
    let graph = task_graph(vec![port("mat_a", ArgCategory::Mmap, 512, Some(1))]);
    let spec = metadata::zip_pkg::spec_from_task_graph(&graph).expect("project");
    assert_eq!(spec.args.len(), 1);
    assert_eq!(spec.args[0].name, "mat_a_0");
}

/// The category is normalized to `"mmap"` when the archive is written, so the
/// fan-out has to survive a real `"cat": "hmap"` round trip through the
/// schema, not just a hand-built `Port`.
#[test]
fn hmap_fans_out_when_read_from_archive_json() {
    let json = r#"{
        "top": "Gemv", "target": "xilinx-hls",
        "tasks": {"Gemv": {"level": "lower", "code": "", "synth": "hls",
            "readable_name": "Gemv", "clock_period": "0",
            "ports": [
                {"cat": "hmap", "name": "mat_a", "type": "int*", "width": 512,
                 "chan_count": 2, "chan_size": 131072},
                {"cat": "mmap", "name": "vec_x", "type": "int*", "width": 512}
            ]}}
    }"#;
    let graph: TaskGraph = serde_json::from_str(json).expect("parse task graph");
    let spec = metadata::zip_pkg::spec_from_task_graph(&graph).expect("project");
    let names: Vec<&str> = spec.args.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["mat_a_0", "mat_a_1", "vec_x"]);
}

/// `tapa-codegen` rejects a zero-channel hmap; fanning out to nothing here
/// would drop the port and shift every later id, which is the exact failure
/// the fan-out exists to prevent.
#[test]
fn zero_channel_hmap_is_rejected() {
    let graph = task_graph(vec![port("mat_a", ArgCategory::Mmap, 512, Some(0))]);
    let err = metadata::zip_pkg::spec_from_task_graph(&graph)
        .expect_err("a zero-channel hmap must be rejected");
    assert!(
        err.to_string().contains("mat_a"),
        "error must name the offending argument; got {err}",
    );
}

#[test]
fn missing_top_task_is_rejected() {
    let mut graph = task_graph(vec![]);
    graph.top = "nonexistent".to_owned();
    let err =
        metadata::zip_pkg::spec_from_task_graph(&graph).expect_err("missing top must be rejected");
    assert!(
        err.to_string().contains("nonexistent"),
        "error must name the missing top task; got {err}",
    );
}
