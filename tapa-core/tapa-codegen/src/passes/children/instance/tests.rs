// In-src unit tests for child `ModuleInstance` assembly.

use super::*;
// stream args and delegate to the real constructor.
fn build_child_instance_test(
    child_task_name: &str,
    instance_name: &str,
    sig: &InstanceSignals,
    args: &BTreeMap<String, Arg>,
    mmap_bindings: &ChildMmapBindings,
    child_rtl: Option<&VerilogModule>,
) -> ModuleInstance {
    let parent_fifos: BTreeSet<String> = args
        .values()
        .filter(|a| {
            matches!(
                a.cat,
                ArgCategory::Istream
                    | ArgCategory::Istreams
                    | ArgCategory::Ostream
                    | ArgCategory::Ostreams
            )
        })
        .filter_map(|a| a.name().map(str::to_owned))
        .collect();
    build_child_instance_with_reset(
        child_task_name,
        instance_name,
        sig,
        args,
        mmap_bindings,
        &parent_fifos,
        None,
        child_rtl,
        Expr::ident(HANDSHAKE_RST_N),
    )
}

#[test]
fn build_child_instance_has_handshake_and_args() {
    use std::collections::BTreeMap;
    let sig = InstanceSignals::new("worker_0", false);
    let mut args = BTreeMap::new();
    args.insert(
        "data_in".to_owned(),
        Arg::named("fifo_0".to_owned(), ArgCategory::Istream),
    );
    args.insert(
        "size".to_owned(),
        Arg::named("n".to_owned(), ArgCategory::Scalar),
    );
    let inst = build_child_instance_test(
        "worker",
        "worker_0",
        &sig,
        &args,
        &ChildMmapBindings::default(),
        None,
    );
    let text = inst.to_string();
    // Should have module name and instance name
    assert!(text.contains("worker worker_0"), "got:\n{text}");
    // Should have handshake ports
    assert!(
        text.contains(".ap_start(worker_0__ap_start)"),
        "got:\n{text}"
    );
    assert!(text.contains(".ap_done(worker_0__ap_done)"), "got:\n{text}");
    // Should have scalar arg connected to per-instance pipeline signal
    assert!(text.contains(".size(worker_0__size)"), "got:\n{text}");
    // Should have istream suffixes
    assert!(text.contains("data_in_s_dout"), "got:\n{text}");
}

#[test]
fn build_child_instance_uses_hls_stream_names_without_child_rtl() {
    use std::collections::BTreeMap;
    let sig = InstanceSignals::new("worker_0", false);
    let mut args = BTreeMap::new();
    args.insert(
        "data_in".to_owned(),
        Arg::named("fifo_0".to_owned(), ArgCategory::Istream),
    );
    args.insert(
        "data_out".to_owned(),
        Arg::named("fifo_1".to_owned(), ArgCategory::Ostream),
    );
    let inst = build_child_instance_test(
        "worker",
        "worker_0",
        &sig,
        &args,
        &ChildMmapBindings::default(),
        None,
    );
    let text = inst.to_string();
    assert!(
        text.contains(".data_in_s_dout(fifo_0_dout)"),
        "got:\n{text}"
    );
    assert!(text.contains(".data_out_s_din(fifo_1_din)"), "got:\n{text}");
}

#[test]
fn build_child_instance_ties_off_ostream_peek_artifact() {
    use std::collections::BTreeMap;
    let sig = InstanceSignals::new("worker_0", false);
    let child_rtl = VerilogModule::parse(concat!(
        "module worker(input wire ap_clk, output wire [32:0] data_out_s_din, ",
        "input wire data_out_s_full_n, output wire data_out_s_write, ",
        "input wire [32:0] data_out_peek); endmodule"
    ))
    .unwrap();
    let mut args = BTreeMap::new();
    args.insert(
        "data_out".to_owned(),
        Arg::named("fifo_1".to_owned(), ArgCategory::Ostream),
    );
    let inst = build_child_instance_test(
        "worker",
        "worker_0",
        &sig,
        &args,
        &ChildMmapBindings::default(),
        Some(&child_rtl),
    );
    let text = inst.to_string();
    assert!(text.contains(".data_out_peek('d0)"), "got:\n{text}");
}

#[test]
fn build_child_instance_sanitizes_indexed_stream_names() {
    use std::collections::BTreeMap;
    let sig = InstanceSignals::new("worker_0", false);
    let child_rtl = VerilogModule::parse(
            "module worker(input wire ap_clk, input wire qs_24_Network_s_dout, input wire qs_24_Network_s_empty_n, output wire qs_24_Network_s_read); endmodule",
        )
        .unwrap();
    let mut args = BTreeMap::new();
    args.insert(
        "qs[24]_Network".to_owned(),
        Arg::named("qs[24]_Network".to_owned(), ArgCategory::Istream),
    );
    let inst = build_child_instance_test(
        "worker",
        "worker_0",
        &sig,
        &args,
        &ChildMmapBindings::default(),
        Some(&child_rtl),
    );
    let text = inst.to_string();
    assert!(
        text.contains(".qs_24_Network_s_dout(qs_24_Network_dout)"),
        "got:\n{text}"
    );
    assert!(
        !text.contains("qs[24]"),
        "indexed names must be sanitized in emitted Verilog:\n{text}"
    );
}

#[test]
fn build_child_instance_connects_istream_peek_inputs() {
    use std::collections::BTreeMap;
    let sig = InstanceSignals::new("switch_0", false);
    let child_rtl = VerilogModule::parse(
        "module switch(\n\
             input wire ap_clk,\n\
             input wire pkt_in_q0_dout,\n\
             input wire pkt_in_q0_empty_n,\n\
             output wire pkt_in_q0_read,\n\
             input wire pkt_in_q0_peek_dout,\n\
             input wire pkt_in_q0_peek_empty_n\n\
             ); endmodule",
    )
    .unwrap();
    let mut args = BTreeMap::new();
    args.insert(
        "pkt_in_q0".to_owned(),
        Arg::named("fifo_0".to_owned(), ArgCategory::Istream),
    );
    let inst = build_child_instance_test(
        "switch",
        "switch_0",
        &sig,
        &args,
        &ChildMmapBindings::default(),
        Some(&child_rtl),
    );
    let text = inst.to_string();
    assert!(
        text.contains(".pkt_in_q0_peek_dout(fifo_0_dout)"),
        "peek dout should reuse the base FIFO dout signal:\n{text}"
    );
    assert!(
        text.contains(".pkt_in_q0_peek_empty_n(fifo_0_empty_n)"),
        "peek empty_n should reuse the base FIFO empty signal:\n{text}"
    );
}

#[test]
fn build_child_instance_connects_array_istream_peek_inputs() {
    use std::collections::BTreeMap;
    let sig = InstanceSignals::new("stage_0", false);
    let child_rtl = VerilogModule::parse(
        "module stage(\n\
             input wire ap_clk,\n\
             input wire in_q_0_dout,\n\
             input wire in_q_0_empty_n,\n\
             output wire in_q_0_read,\n\
             input wire in_q_peek_0_dout,\n\
             input wire in_q_peek_0_empty_n\n\
             ); endmodule",
    )
    .unwrap();
    let mut args = BTreeMap::new();
    args.insert(
        "in_q[0]".to_owned(),
        Arg::named("fifo[0]".to_owned(), ArgCategory::Istream),
    );
    let inst = build_child_instance_test(
        "stage",
        "stage_0",
        &sig,
        &args,
        &ChildMmapBindings::default(),
        Some(&child_rtl),
    );
    let text = inst.to_string();
    assert!(
        text.contains(".in_q_peek_0_dout(fifo_0_dout)"),
        "array peek dout should use compatible name ordering:\n{text}"
    );
    assert!(
        text.contains(".in_q_peek_0_empty_n(fifo_0_empty_n)"),
        "array peek empty_n should use compatible name ordering:\n{text}"
    );
}

#[test]
fn build_child_instance_passes_stream_ports_through_with_s_infix() {
    // A middle/upper task that passes its own stream PORT straight through
    // to a child (no intervening FIFO) must connect the child's `_s`/`_peek`
    // ports to the parent's identically-named ports. The signal must NOT use
    // the bare `{name}{suffix}` FIFO-wire spelling (which would be an
    // undeclared 1-bit implicit net and silently drop the stream data).
    use std::collections::{BTreeMap, BTreeSet};
    let child_rtl = VerilogModule::parse(
        "module Add(\n\
             input wire ap_clk,\n\
             input wire [32:0] a_int_s_dout,\n\
             input wire a_int_s_empty_n,\n\
             output wire a_int_s_read,\n\
             input wire [32:0] a_int_peek_dout,\n\
             input wire a_int_peek_empty_n,\n\
             output wire a_int_peek_read,\n\
             output wire [32:0] c_int_s_din,\n\
             input wire c_int_s_full_n,\n\
             output wire c_int_s_write\n\
             ); endmodule",
    )
    .unwrap();
    let sig = InstanceSignals::new("Add_0", false);
    let mut args = BTreeMap::new();
    args.insert(
        "a_int".to_owned(),
        Arg::named("a_ext".to_owned(), ArgCategory::Istream),
    );
    args.insert(
        "c_int".to_owned(),
        Arg::named("c_ext".to_owned(), ArgCategory::Ostream),
    );
    let parent_rtl = VerilogModule::parse(
        "module Mid(\n\
             input wire [32:0] a_ext_s_dout,\n\
             input wire a_ext_s_empty_n,\n\
             output wire a_ext_s_read,\n\
             input wire [32:0] a_ext_peek_dout,\n\
             input wire a_ext_peek_empty_n,\n\
             output wire [32:0] c_ext_s_din,\n\
             input wire c_ext_s_full_n,\n\
             output wire c_ext_s_write\n\
             ); endmodule",
    )
    .unwrap();
    let parent_fifos: BTreeSet<String> = BTreeSet::new();
    let inst = build_child_instance_with_reset(
        "Add",
        "Add_0",
        &sig,
        &args,
        &ChildMmapBindings::default(),
        &parent_fifos,
        Some(&parent_rtl),
        Some(&child_rtl),
        Expr::ident(HANDSHAKE_RST_N),
    );
    let text = inst.to_string();
    // istream passthrough -> parent port `a_ext_s_dout` (with `_s`).
    assert!(
        text.contains(".a_int_s_dout(a_ext_s_dout)"),
        "istream passthrough must bind to the `_s` port, got:\n{text}"
    );
    assert!(
        !text.contains("a_ext_dout"),
        "bare `a_ext_dout` is an undeclared FIFO-style wire, got:\n{text}"
    );
    // istream peek passthrough -> parent port `a_ext_peek_dout`.
    assert!(
        text.contains(".a_int_peek_dout(a_ext_peek_dout)"),
        "peek passthrough must bind to the `_peek` port, got:\n{text}"
    );
    // ostream passthrough -> parent port `c_ext_s_din`.
    assert!(
        text.contains(".c_int_s_din(c_ext_s_din)"),
        "ostream passthrough must bind to the `_s` port, got:\n{text}"
    );
}

#[test]
fn build_child_instance_passes_array_stream_ports_through_without_infix() {
    // Array stream elements spell the parent's Vitis HLS ports WITHOUT an
    // infix (`in_q_0_dout`, `in_q_peek_0_dout`), unlike the scalar `_s` /
    // `_peek` convention. A passthrough must resolve to those exact
    // parent port names; the hardcoded `_s`/`_peek` spelling would be an
    // undeclared implicit net and deadlock the simulation.
    use std::collections::{BTreeMap, BTreeSet};
    let child_rtl = VerilogModule::parse(
        "module Inner(\n\
             input wire ap_clk,\n\
             input wire [64:0] in_q0_0_dout,\n\
             input wire in_q0_0_empty_n,\n\
             output wire in_q0_0_read,\n\
             input wire [64:0] in_q0_peek_0_dout,\n\
             input wire in_q0_peek_0_empty_n,\n\
             output wire [64:0] out_q_0_din,\n\
             input wire out_q_0_full_n,\n\
             output wire out_q_0_write\n\
             ); endmodule",
    )
    .unwrap();
    let parent_rtl = VerilogModule::parse(
        "module Stage(\n\
             input wire ap_clk,\n\
             input wire [64:0] in_q_0_dout,\n\
             input wire in_q_0_empty_n,\n\
             output wire in_q_0_read,\n\
             input wire [64:0] in_q_peek_0_dout,\n\
             input wire in_q_peek_0_empty_n,\n\
             output wire [64:0] out_q_0_din,\n\
             input wire out_q_0_full_n,\n\
             output wire out_q_0_write\n\
             ); endmodule",
    )
    .unwrap();
    let sig = InstanceSignals::new("Inner_0", false);
    let mut args = BTreeMap::new();
    args.insert(
        "in_q0[0]".to_owned(),
        Arg::named("in_q[0]".to_owned(), ArgCategory::Istream),
    );
    args.insert(
        "out_q[0]".to_owned(),
        Arg::named("out_q[0]".to_owned(), ArgCategory::Ostream),
    );
    let parent_fifos: BTreeSet<String> = BTreeSet::new();
    let inst = build_child_instance_with_reset(
        "Inner",
        "Inner_0",
        &sig,
        &args,
        &ChildMmapBindings::default(),
        &parent_fifos,
        Some(&parent_rtl),
        Some(&child_rtl),
        Expr::ident(HANDSHAKE_RST_N),
    );
    let text = inst.to_string();
    assert!(
        text.contains(".in_q0_0_dout(in_q_0_dout)"),
        "array istream passthrough must bind to `in_q_0_dout`, got:\n{text}"
    );
    assert!(
        text.contains(".in_q0_peek_0_dout(in_q_peek_0_dout)"),
        "array istream peek passthrough must bind to `in_q_peek_0_dout`, got:\n{text}"
    );
    assert!(
        text.contains(".out_q_0_din(out_q_0_din)"),
        "array ostream passthrough must bind to `out_q_0_din`, got:\n{text}"
    );
    assert!(
        !text.contains("in_q_0_s_dout") && !text.contains("in_q_0_peek_dout"),
        "hardcoded `_s`/`_peek` spellings are undeclared implicit nets, got:\n{text}"
    );
}

#[test]
fn build_child_instance_sanitizes_indexed_mmap_signals() {
    use std::collections::BTreeMap;
    let sig = InstanceSignals::new("worker_0", false);
    let mut args = BTreeMap::new();
    args.insert(
        "mem".to_owned(),
        Arg::named("chan[0]".to_owned(), ArgCategory::Mmap),
    );
    let inst = build_child_instance_test(
        "worker",
        "worker_0",
        &sig,
        &args,
        &ChildMmapBindings::default(),
        None,
    );
    let text = inst.to_string();
    assert!(
        text.contains(".m_axi_mem_ARADDR(m_axi_chan_0_ARADDR)"),
        "got:\n{text}"
    );
    assert!(!text.contains("m_axi_chan[0]"), "got:\n{text}");
}

#[test]
fn build_child_instance_connects_async_mmap_stream_ports() {
    use std::collections::BTreeMap;
    let sig = InstanceSignals::new("copy_0", false);
    let child_rtl = VerilogModule::parse(
        "module copy(\n\
             input wire ap_clk,\n\
             output wire [63:0] mem_read_addr_s_din,\n\
             input wire mem_read_addr_s_full_n,\n\
             output wire mem_read_addr_s_write,\n\
             input wire [63:0] mem_read_addr_offset,\n\
             input wire [512:0] mem_read_data_s_dout,\n\
             input wire mem_read_data_s_empty_n,\n\
             output wire mem_read_data_s_read,\n\
             input wire [512:0] mem_read_data_peek_dout,\n\
             input wire mem_read_data_peek_empty_n,\n\
             output wire mem_write_addr_s_write,\n\
             input wire [63:0] mem_write_addr_offset,\n\
             output wire [512:0] mem_write_data_s_din,\n\
             input wire mem_write_data_s_full_n,\n\
             input wire [8:0] mem_write_resp_s_dout,\n\
             input wire mem_write_resp_s_empty_n,\n\
             output wire mem_write_resp_s_read\n\
             ); endmodule",
    )
    .unwrap();
    let mut args = BTreeMap::new();
    args.insert(
        "mem".to_owned(),
        Arg::named("chan[0]".to_owned(), ArgCategory::AsyncMmap),
    );
    let inst = build_child_instance_test(
        "copy",
        "copy_0",
        &sig,
        &args,
        &ChildMmapBindings::default(),
        Some(&child_rtl),
    );
    let text = inst.to_string();
    assert!(
        text.contains(".mem_read_addr_s_din(chan_0_read_addr__din)"),
        "read address stream should connect to async_mmap bridge wires:\n{text}"
    );
    assert!(
        text.contains(".mem_read_data_s_dout({1'b0, chan_0_read_data__dout})"),
        "read data stream should prepend a false EOT bit:\n{text}"
    );
    assert!(
        text.contains(".mem_read_data_peek_dout({1'b0, chan_0_read_data__dout})"),
        "read data peek should mirror the bridge data signal:\n{text}"
    );
    assert!(
        text.contains(".mem_write_resp_s_dout({1'b0, chan_0_write_resp__dout})"),
        "write response stream should prepend a false EOT bit:\n{text}"
    );
    assert!(
        text.contains(".mem_read_addr_offset(copy_0__mem_offset)"),
        "read address offset should use the per-instance offset pipeline:\n{text}"
    );
    assert!(
        text.contains(".mem_write_addr_offset(copy_0__mem_offset)"),
        "write address offset should use the per-instance offset pipeline:\n{text}"
    );
    assert!(
        !text.contains(".m_axi_mem_"),
        "async_mmap children should not be wired as direct AXI children:\n{text}"
    );
}

#[test]
fn build_child_instance_connects_async_mmap_slot_axi_ports() {
    use std::collections::BTreeMap;
    let sig = InstanceSignals::new("SLOT_X0Y2_SLOT_X0Y2_0", false);
    let child_rtl = VerilogModule::parse(
        "module SLOT_X0Y2_SLOT_X0Y2(\n\
             input wire ap_clk,\n\
             input wire [63:0] mem_Copy_0_offset\n\
             ); endmodule",
    )
    .unwrap();
    let mut args = BTreeMap::new();
    args.insert(
        "mem_Copy_0".to_owned(),
        Arg::named("chan[0]".to_owned(), ArgCategory::AsyncMmap),
    );
    let inst = build_child_instance_test(
        "SLOT_X0Y2_SLOT_X0Y2",
        "SLOT_X0Y2_SLOT_X0Y2_0",
        &sig,
        &args,
        &ChildMmapBindings::default(),
        Some(&child_rtl),
    );
    let text = inst.to_string();
    assert!(
        text.contains(".mem_Copy_0_offset(SLOT_X0Y2_SLOT_X0Y2_0__mem_Copy_0_offset)"),
        "slot async mmap offset should connect to the per-instance offset pipeline:\n{text}"
    );
    assert!(
            text.contains(".m_axi_mem_Copy_0_AWADDR(m_axi_chan_0_AWADDR)"),
            "slot async mmap AXI ports should connect to the parent channel once the slot exposes the direct offset:\n{text}"
        );
    assert!(
        text.contains(".m_axi_mem_Copy_0_AWVALID(m_axi_chan_0_AWVALID)"),
        "slot async mmap binding should emit the full direct AXI bundle:\n{text}"
    );
}

#[test]
fn build_child_instance_uses_vitis_2025_offset_spelling_when_present() {
    use std::collections::BTreeMap;
    // Vitis HLS 2025.1+ names the `offset=direct` scalar `<port>_r`
    // where every earlier version emitted `<port>_offset`. The pin name
    // must follow the parsed child RTL, while the parent-side wire
    // keeps the conventional `_offset` spelling.
    let sig = InstanceSignals::new("Mmap2Stream_0", false);
    let child_rtl = VerilogModule::parse(
        "module Mmap2Stream(input wire ap_clk, input wire [63:0] mmap_r, \
         output wire [63:0] m_axi_mmap_AWADDR); endmodule",
    )
    .unwrap();
    let mut args = BTreeMap::new();
    args.insert(
        "mmap".to_owned(),
        Arg::named("a".to_owned(), ArgCategory::Mmap),
    );
    let inst = build_child_instance_test(
        "Mmap2Stream",
        "Mmap2Stream_0",
        &sig,
        &args,
        &ChildMmapBindings::default(),
        Some(&child_rtl),
    );
    let text = inst.to_string();
    assert!(
        text.contains(".mmap_r(Mmap2Stream_0__mmap_offset)"),
        "2025-style child offset port must be pinned by its real name:\n{text}"
    );
    assert!(
        !text.contains(".mmap_offset("),
        "no phantom conventional pin when the child only has `_r`:\n{text}"
    );
}

/// The direct-offset probe order is pinned by the shared naming fixture:
/// `frt-cosim` testbenches probe the same candidate list, and both tests
/// reading one file keeps the two implementations in lockstep (the 2025.2
/// `_offset` -> `_r` rename had to be fixed in both independently).
#[test]
fn direct_mmap_offset_port_follows_naming_fixture() {
    let fixture = include_str!("../../../../../tapa-ir/testdata/naming_conventions.tsv");
    let mut checked = 0;
    for line in fixture
        .lines()
        .filter(|line| line.starts_with("direct_offset_port\t"))
    {
        let fields: Vec<&str> = line.split('\t').collect();
        let (base, candidates) = (fields[1], &fields[2..]);
        assert!(candidates.len() >= 2, "probe list needs candidates: {line}");
        // A child that declares exactly one candidate gets that pin.
        for expected in candidates {
            let rtl = VerilogModule::parse(&format!(
                "module Child(input wire [63:0] {expected}); endmodule"
            ))
            .unwrap();
            assert_eq!(
                direct_mmap_offset_port(Some(&rtl), base),
                *expected,
                "line: {line}"
            );
        }
        // Several candidates resolve in fixture probe order; a child
        // without any keeps the first (conventional) spelling.
        let all = candidates
            .iter()
            .map(|candidate| format!("input wire [63:0] {candidate}"))
            .collect::<Vec<_>>()
            .join(", ");
        let rtl = VerilogModule::parse(&format!("module Child({all}); endmodule")).unwrap();
        assert_eq!(
            direct_mmap_offset_port(Some(&rtl), base),
            candidates[0],
            "line: {line}"
        );
        assert_eq!(
            direct_mmap_offset_port(None, base),
            candidates[0],
            "line: {line}"
        );
        checked += 1;
    }
    assert!(
        checked >= 1,
        "fixture lost its direct_offset_port production"
    );
}

#[test]
fn build_child_instance_keeps_conventional_offset_spelling() {
    use std::collections::BTreeMap;
    let sig = InstanceSignals::new("Mmap2Stream_0", false);
    let child_rtl = VerilogModule::parse(
        "module Mmap2Stream(input wire ap_clk, input wire [63:0] mmap_offset, \
         output wire [63:0] m_axi_mmap_AWADDR); endmodule",
    )
    .unwrap();
    let mut args = BTreeMap::new();
    args.insert(
        "mmap".to_owned(),
        Arg::named("a".to_owned(), ArgCategory::Mmap),
    );
    let inst = build_child_instance_test(
        "Mmap2Stream",
        "Mmap2Stream_0",
        &sig,
        &args,
        &ChildMmapBindings::default(),
        Some(&child_rtl),
    );
    let text = inst.to_string();
    assert!(
        text.contains(".mmap_offset(Mmap2Stream_0__mmap_offset)"),
        "pre-2025 child offset port keeps the conventional pin:\n{text}"
    );
}
