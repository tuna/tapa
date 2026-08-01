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
        stream_depth: None,
        mmap_addr_width: None,
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

/// Archives written before the schema grew per-port cosim metadata carry no
/// `depth` / `addr_width`; projection must keep giving their streams and
/// mmaps the values those archives have always been simulated with, so the
/// argument shape is bit-for-bit stable for old artifacts.
#[test]
fn legacy_archive_ports_fall_back_to_the_historical_16_and_64() {
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

/// Mixed archives keep the two paths side by side: a stamped port projects
/// its schema value while an unstamped one keeps the legacy fallback, and a
/// fan-out (`hmap` / plural streams) carries the port's value to every
/// channel it spawns.
#[test]
fn stamped_and_unstamped_ports_mix_across_fanout() {
    let mut hmap = port("mat_a", ArgCategory::Mmap, 512, Some(2));
    hmap.mmap_addr_width = Some(48);
    let mut streams = port("q", ArgCategory::Ostreams, 32, Some(2));
    streams.stream_depth = Some(4);
    let graph = task_graph(vec![
        hmap,
        port("vec_x", ArgCategory::Mmap, 512, None),
        streams,
        port("plain", ArgCategory::Istream, 32, None),
    ]);
    let spec = metadata::zip_pkg::spec_from_task_graph(&graph).expect("project");
    let kinds: Vec<&ArgKind> = spec.args.iter().map(|a| &a.kind).collect();
    assert_eq!(
        kinds,
        vec![
            &ArgKind::Mmap {
                data_width: 512,
                addr_width: 48,
            },
            &ArgKind::Mmap {
                data_width: 512,
                addr_width: 48,
            },
            &ArgKind::Mmap {
                data_width: 512,
                addr_width: 64,
            },
            &ArgKind::Stream {
                width: 32,
                depth: 4,
                dir: StreamDir::Out,
                protocol: StreamProtocol::ApFifo,
            },
            &ArgKind::Stream {
                width: 32,
                depth: 4,
                dir: StreamDir::Out,
                protocol: StreamProtocol::ApFifo,
            },
            &ArgKind::Stream {
                width: 32,
                depth: 16,
                dir: StreamDir::In,
                protocol: StreamProtocol::ApFifo,
            },
        ],
        "every fan-out channel inherits its port's stamped value; unstamped ports keep the fallback",
    );
}

/// The stamped values travel through the archive schema, not just hand-built
/// `Port`s: a `tapa.json` fragment carrying the fields projects them.
#[test]
fn stamped_metadata_survives_the_archive_json_round_trip() {
    let json = r#"{
        "top": "K", "target": "xilinx-hls",
        "tasks": {"K": {"level": "lower", "code": "", "synth": "hls",
            "readable_name": "K", "clock_period": "0",
            "ports": [
                {"cat": "mmap", "name": "a", "type": "int*", "width": 512,
                 "mmap_addr_width": 64},
                {"cat": "istream", "name": "s", "type": "int", "width": 32,
                 "stream_depth": 16}
            ]}}
    }"#;
    let graph: TaskGraph = serde_json::from_str(json).expect("parse task graph");
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

/// The category is normalized to `"mmap"` when the archive is written --
/// hmap-ness travels via `chan_count`/`chan_size`, not the wire category --
/// so the fan-out has to survive a real archive-JSON round trip through the
/// schema, not just a hand-built `Port`.
#[test]
fn hmap_fans_out_when_read_from_archive_json() {
    let json = r#"{
        "top": "Gemv", "target": "xilinx-hls",
        "tasks": {"Gemv": {"level": "lower", "code": "", "synth": "hls",
            "readable_name": "Gemv", "clock_period": "0",
            "ports": [
                {"cat": "mmap", "name": "mat_a", "type": "int*", "width": 512,
                 "chan_count": 2, "chan_size": 131072},
                {"cat": "mmap", "name": "vec_x", "type": "int*", "width": 512}
            ]}}
    }"#;
    let graph: TaskGraph = serde_json::from_str(json).expect("parse task graph");
    let spec = metadata::zip_pkg::spec_from_task_graph(&graph).expect("project");
    let names: Vec<&str> = spec.args.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["mat_a_0", "mat_a_1", "vec_x"]);
}

/// The literal `"hmap"` category is a frontend concept only: it never reaches
/// the archive schema, and `tapa-ir` must keep rejecting it loudly rather
/// than silently binding it as a plain mmap.
#[test]
fn hmap_category_is_rejected_by_the_schema() {
    let json = r#"{
        "top": "Gemv", "target": "xilinx-hls",
        "tasks": {"Gemv": {"level": "lower", "code": "", "synth": "hls",
            "readable_name": "Gemv", "clock_period": "0",
            "ports": [
                {"cat": "hmap", "name": "mat_a", "type": "int*", "width": 512,
                 "chan_count": 2, "chan_size": 131072}
            ]}}
    }"#;
    let err = serde_json::from_str::<TaskGraph>(json)
        .expect_err("the hmap wire category must be rejected");
    assert!(
        err.to_string().contains("unknown category: hmap"),
        "error must name the rejected category; got {err}",
    );
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

/// A top-level `mmaps<T, N>` is expanded by the frontend into N ports named
/// `chan[0]`, `chan[1]`, ... -- but the generated RTL, the `kernel.xml` args
/// and the `_offset` registers all spell them `chan_0`. These names are what
/// the testbench binds by (`lookup_register_offset`, `detect_axi_id_width`,
/// the shm buffer), so a bracketed name matches nothing. Exercised by
/// `tests/apps/bandwidth`.
#[test]
fn expanded_mmaps_channels_use_rtl_names() {
    let graph = task_graph(vec![
        port("chan[0]", ArgCategory::Mmap, 512, None),
        port("chan[1]", ArgCategory::Mmap, 512, None),
        port("n", ArgCategory::Scalar, 64, None),
    ]);
    let spec = metadata::zip_pkg::spec_from_task_graph(&graph).expect("project");
    let names: Vec<&str> = spec.args.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["chan_0", "chan_1", "n"],
        "mmaps channels bind by their RTL names, not the schema's brackets",
    );
}

/// The bracket collapsing applies to every category that can carry an
/// expanded name, not just mmaps -- a stream array's channel suffix is
/// appended to the *sanitized* base.
#[test]
fn expanded_stream_channels_use_rtl_names() {
    let graph = task_graph(vec![
        port("s[0]", ArgCategory::Istream, 32, None),
        port("q[1]", ArgCategory::Ostreams, 32, Some(2)),
    ]);
    let spec = metadata::zip_pkg::spec_from_task_graph(&graph).expect("project");
    let names: Vec<&str> = spec.args.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["s_0_s", "q_1_0", "q_1_1"]);
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
