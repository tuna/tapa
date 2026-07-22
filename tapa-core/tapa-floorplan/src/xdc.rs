//! Pblock XDC emission: turn a [`FloorplanResult`] plus its [`Device`] into the
//! `create_pblock`/`resize_pblock`/`add_cells_to_pblock` constraints Vivado
//! applies during implementation.
//!
//! Cells are matched with hierarchical wildcards so they still resolve once
//! `v++` places the kernel below a platform prefix and synthesis flattens names.

use std::collections::{BTreeMap, BTreeSet};

use tapa_ir::port::{sanitize_array_name, sanitize_identifier_name};
use tapa_ir::{
    axi_pipeline_instance_name, control_pipeline_instance_name, FloorplanResult, RoutedChannel,
};

use crate::device::model::{Coor, Device};

/// Render the floorplan as XDC pblock constraints, terminated by a newline.
#[must_use]
pub fn emit_xdc(result: &FloorplanResult, device: &Device) -> String {
    // A crossing FIFO is split into independently constrained Head/Body/Tail
    // hierarchy below. Constraining its monolithic parent as well would force
    // every stage back into the FIFO vertex's old placement pblock.
    let routed_streams: BTreeSet<&str> = result
        .routes
        .iter()
        .filter_map(|route| match &route.channel {
            RoutedChannel::Stream { fifo } => Some(fifo.as_str()),
            RoutedChannel::Axi { .. } | RoutedChannel::Control { .. } => None,
        })
        .collect();

    // Group cell matches by physical region so each pblock is created once.
    let mut by_region: BTreeMap<String, Vec<CellMatch>> = BTreeMap::new();
    for (instance, region) in &result.regions {
        if routed_streams.contains(instance.as_str()) {
            continue;
        }
        by_region
            .entry(canonical_pblock_name(region))
            .or_default()
            .push(CellMatch {
                pattern: cell_name_regex(instance),
                description: instance.clone(),
            });
    }

    for route in &result.routes {
        let (pipeline_instance, description) = match &route.channel {
            RoutedChannel::Stream { fifo } => {
                (format!("{}_fifo", sanitize_array_name(fifo)), fifo.clone())
            }
            RoutedChannel::Axi {
                endpoint, channel, ..
            } => (
                axi_pipeline_instance_name(endpoint, *channel),
                format!(
                    "{}.{} {}",
                    endpoint.instance,
                    endpoint.port,
                    channel.rtl_name()
                ),
            ),
            RoutedChannel::Control { instance, channel } => (
                control_pipeline_instance_name(instance, *channel),
                format!("{instance} {}", channel.rtl_name()),
            ),
        };
        let (Some(head_region), Some(tail_region)) = (route.route.first(), route.route.last())
        else {
            continue;
        };

        by_region
            .entry(canonical_pblock_name(head_region))
            .or_default()
            .push(CellMatch {
                pattern: pipeline_head_regex(&pipeline_instance),
                description: format!("{description} Head"),
            });
        for (index, region) in route.reg_regions.iter().enumerate() {
            by_region
                .entry(canonical_pblock_name(region))
                .or_default()
                .push(CellMatch {
                    pattern: pipeline_body_regex(&pipeline_instance, index),
                    description: format!("{description} Body {index}"),
                });
        }
        by_region
            .entry(canonical_pblock_name(tail_region))
            .or_default()
            .push(CellMatch {
                pattern: pipeline_tail_regex(&pipeline_instance),
                description: format!("{description} Tail"),
            });
    }

    let mut lines: Vec<String> = Vec::new();
    for (region, matches) in &by_region {
        add_region_pblock(&mut lines, region, matches, device);
    }

    let mut text = lines.join("\n");
    text.push('\n');
    text
}

struct CellMatch {
    pattern: String,
    description: String,
}

fn add_region_pblock(
    lines: &mut Vec<String>,
    region: &str,
    matches: &[CellMatch],
    device: &Device,
) {
    let slot_ranges = region_slot_pblock_ranges(device, region);
    lines.push(format!("create_pblock {region}"));
    lines.extend([
        format!("set_property EXCLUDE_PLACEMENT 0 [get_pblocks {region}]"),
        format!("set_property CONTAIN_ROUTING 0 [get_pblocks {region}]"),
        // IS_SOFT must follow EXCLUDE_PLACEMENT, which can reset it.
        format!("set_property IS_SOFT 1 [get_pblocks {region}]"),
    ]);
    if let Some(parent) = &device.user_pblock_name {
        lines.push(format!(
            "set_property PARENT {parent} [get_pblocks {region}]"
        ));
    }
    add_slot_ranges(lines, region, &slot_ranges);
    for cell_match in matches {
        // `-quiet` suppresses Vivado's version-specific empty-query
        // diagnostic; the explicit check below is the stable DRC.
        lines.push(format!(
            "set cells [get_cells -quiet -hierarchical -regexp -filter {{NAME =~ \"{}\"}}]",
            cell_match.pattern
        ));
        lines.push(format!(
            "if {{![llength $cells]}} {{ error \"TAPA floorplan ERROR: expected cell `{}` was not \
             found\" }}",
            tcl_double_quote_escape(&cell_match.description),
        ));
        lines.push(format!("add_cells_to_pblock {region} $cells"));
    }
    if let Some(parent) = &device.user_pblock_name {
        add_parent_clip(lines, region, parent);
    }
}

/// Escape text embedded in a Tcl double-quoted diagnostic without changing
/// what Vivado displays. Brackets matter for flattened array names: leaving
/// them bare would invoke Tcl command substitution while reporting the error.
fn tcl_double_quote_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\\' | '"' | '$' | '[' => escaped.push('\\'),
            '\n' => {
                escaped.push_str("\\n");
                continue;
            }
            '\r' => {
                escaped.push_str("\\r");
                continue;
            }
            _ => {}
        }
        escaped.push(character);
    }
    escaped
}

/// A Vivado `-regexp` `NAME` pattern matching `instance`'s RTL cell as a
/// *complete* hierarchy path component: preceded by the netlist root or a `/`,
/// and followed by the end of the name or a `/` (its descendant cells).
///
/// Two transforms bridge the graph name to the netlist name codegen emits:
///   * `sanitize_identifier_name` applies codegen's Verilog identifier rules;
///   * an optional `_fifo` suffix matches FIFO and handshake-pipeline instances,
///     which codegen names `{sanitized}_fifo`, while leaf tasks carry no suffix.
///
/// Anchoring keeps `PEG_Xvec_1` from also capturing `PEG_Xvec_10..19`.
fn cell_name_regex(instance: &str) -> String {
    format!(
        "^(.*/)?{}(_fifo)?(/.*)?$",
        regex_escape(&sanitize_identifier_name(instance))
    )
}

/// Match the two source-slot Head cells below a generated stream pipeline.
/// The gate is combinational, while `TAPA_HS_HEAD` owns the actual ready,
/// valid, and data registers. Keeping both in one match also survives Vivado
/// retaining the gate as a separate hierarchy cell.
fn pipeline_head_regex(instance: &str) -> String {
    format!(
        "^(.*/)?{}/TAPA_HS_HEAD(_GATE)?(/.*)?$",
        regex_escape(instance)
    )
}

/// Match one generated Body register and all of its descendants.
///
/// Vivado may render a named generate scope separator as either `/` or `.`, so
/// the middle `.*` deliberately accepts both while the escaped index and
/// complete child name keep the match exact.
fn pipeline_body_regex(instance: &str, index: usize) -> String {
    format!(
        "^(.*/)?{}/TAPA_HS_BODY\\[{index}\\].*TAPA_HS_BODY_REG(/.*)?$",
        regex_escape(instance)
    )
}

/// Match the destination-slot Tail FIFO and every cell below it.
fn pipeline_tail_regex(instance: &str) -> String {
    format!("^(.*/)?{}/TAPA_HS_TAIL(/.*)?$", regex_escape(instance))
}

/// Backslash-escape every regex metacharacter so an instance name is matched
/// literally. `[` and `]` matter most — they appear in flattened FIFO indices.
fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if matches!(
            c,
            '.' | '^' | '$' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The pblock operations for every atomic slot a region covers.
fn region_slot_pblock_ranges<'a>(device: &'a Device, region: &str) -> Vec<&'a [String]> {
    let Some(coor) = parse_region_or_slot(region) else {
        return Vec::new();
    };
    let mut ranges = Vec::new();
    for (x, y) in coor.all_slot_coors() {
        if let Some(slot) = device.slot(x, y) {
            ranges.push(slot.pblock_ranges.as_slice());
        }
    }
    ranges
}

fn add_slot_ranges(lines: &mut Vec<String>, region: &str, slots: &[&[String]]) {
    if let [ranges] = slots {
        add_resize_operations(lines, region, ranges);
        return;
    }
    if slots
        .iter()
        .all(|ranges| ranges.iter().all(|range| !range.starts_with("-remove ")))
    {
        for ranges in slots {
            add_resize_operations(lines, region, ranges);
        }
        return;
    }

    // Build each atomic slot independently before unioning its derived ranges.
    // This keeps a slot's `-remove` clauses from subtracting a neighboring slot.
    for (index, ranges) in slots.iter().enumerate() {
        let temporary = format!("TAPA_SLOT_UNION_{region}_{index}");
        lines.push(format!("create_pblock {temporary}"));
        add_resize_operations(lines, &temporary, ranges);
        lines.push(format!(
            "set slot_ranges [get_property DERIVED_RANGES [get_pblocks {temporary}]]"
        ));
        lines.push("if {$slot_ranges ne \"\"} {".to_string());
        lines.push(format!("  resize_pblock {region} -add $slot_ranges"));
        lines.push('}'.to_string());
        lines.push(format!("delete_pblock -quiet {temporary}"));
    }
}

fn add_resize_operations(lines: &mut Vec<String>, pblock: &str, ranges: &[String]) {
    for range in ranges {
        if range.starts_with("-add ") || range.starts_with("-remove ") {
            lines.push(format!("resize_pblock {pblock} {range}"));
        } else if range.starts_with('{') {
            lines.push(format!("resize_pblock {pblock} -add {range}"));
        } else {
            lines.push(format!("resize_pblock {pblock} -add {{{range}}}"));
        }
    }
}

/// Remove every derived range of `region` that falls outside `parent`.
fn add_parent_clip(lines: &mut Vec<String>, region: &str, parent: &str) {
    let temporary = format!("TAPA_PARENT_CLIP_{region}");
    lines.push(format!("create_pblock {temporary}"));
    lines.push(format!(
        "set derived_ranges [get_property DERIVED_RANGES [get_pblocks {region}]]"
    ));
    lines.push("if {$derived_ranges ne \"\"} {".to_string());
    lines.push(format!("  resize_pblock {temporary} -add $derived_ranges"));
    lines.push('}'.to_string());
    lines.push(format!(
        "set derived_ranges [get_property DERIVED_RANGES [get_pblocks {parent}]]"
    ));
    lines.push("if {$derived_ranges ne \"\"} {".to_string());
    lines.push(format!(
        "  resize_pblock {temporary} -remove $derived_ranges"
    ));
    lines.push('}'.to_string());
    lines.push(format!(
        "set derived_ranges [get_property DERIVED_RANGES [get_pblocks {temporary}]]"
    ));
    lines.push("if {$derived_ranges ne \"\"} {".to_string());
    lines.push(format!("  resize_pblock {region} -remove $derived_ranges"));
    lines.push('}'.to_string());
    lines.push(format!("delete_pblock -quiet {temporary}"));
}

/// Pipeline routes use compact `SLOT_XxYy` tags while placement uses rectangle
/// tags. Canonicalizing both to the rectangle spelling prevents duplicate,
/// overlapping pblocks for the same atomic slot.
fn canonical_pblock_name(region: &str) -> String {
    parse_region_or_slot(region).map_or_else(|| region.to_string(), |coor| coor.region_name())
}

fn parse_region_or_slot(region: &str) -> Option<Coor> {
    if let Some(coor) = Coor::from_region_name(region) {
        return Some(coor);
    }
    let rest = region.strip_prefix("SLOT_X")?;
    let (x, y) = rest.split_once('Y')?;
    Some(Coor::slot(x.parse().ok()?, y.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::select::select_device;
    use std::collections::BTreeMap as Map;
    use tapa_ir::{
        control_pipeline_instance_name, global_controller_instance_name,
        local_controller_instance_name, Area, AxiChannel, AxiEndpoint, ControlChannel, MemoryBank,
        MemoryKind, PipelineRoute, PipelineScheme,
    };

    #[test]
    #[allow(
        clippy::literal_string_with_formatting_args,
        reason = "the golden XDC contains literal Tcl braces, not format args"
    )]
    fn golden_xdc_for_a_colocated_design() {
        let result = FloorplanResult {
            device: "u250".to_string(),
            grid: (2, 4),
            regions: Map::from([
                ("A_0".to_string(), "SLOT_X0Y0_TO_SLOT_X0Y0".to_string()),
                ("B_0".to_string(), "SLOT_X0Y0_TO_SLOT_X0Y0".to_string()),
                (
                    "fifo_VecAdd".to_string(),
                    "SLOT_X0Y0_TO_SLOT_X0Y0".to_string(),
                ),
            ]),
            routes: Vec::new(),
            slot_usage: Map::from([(
                "SLOT_X0Y0_TO_SLOT_X0Y0".to_string(),
                Area {
                    lut: 150,
                    ff: 260,
                    bram_18k: 0,
                    dsp: 0,
                    uram: 0,
                },
            )]),
        };
        let device = select_device("u250").expect("u250");

        let expected = "\
create_pblock SLOT_X0Y0_TO_SLOT_X0Y0
set_property EXCLUDE_PLACEMENT 0 [get_pblocks SLOT_X0Y0_TO_SLOT_X0Y0]
set_property CONTAIN_ROUTING 0 [get_pblocks SLOT_X0Y0_TO_SLOT_X0Y0]
set_property IS_SOFT 1 [get_pblocks SLOT_X0Y0_TO_SLOT_X0Y0]
resize_pblock SLOT_X0Y0_TO_SLOT_X0Y0 -add {CLOCKREGION_X0Y0:CLOCKREGION_X3Y3}
set cells [get_cells -quiet -hierarchical -regexp -filter {NAME =~ \"^(.*/)?A_0(_fifo)?(/.*)?$\"}]
if {![llength $cells]} { error \"TAPA floorplan ERROR: expected cell `A_0` was not found\" }
add_cells_to_pblock SLOT_X0Y0_TO_SLOT_X0Y0 $cells
set cells [get_cells -quiet -hierarchical -regexp -filter {NAME =~ \"^(.*/)?B_0(_fifo)?(/.*)?$\"}]
if {![llength $cells]} { error \"TAPA floorplan ERROR: expected cell `B_0` was not found\" }
add_cells_to_pblock SLOT_X0Y0_TO_SLOT_X0Y0 $cells
set cells [get_cells -quiet -hierarchical -regexp -filter {NAME =~ \"^(.*/)?fifo_VecAdd(_fifo)?(/.*)?$\"}]
if {![llength $cells]} { error \"TAPA floorplan ERROR: expected cell `fifo_VecAdd` was not found\" }
add_cells_to_pblock SLOT_X0Y0_TO_SLOT_X0Y0 $cells
";
        assert_eq!(emit_xdc(&result, &device), expected);
    }

    #[test]
    #[allow(
        clippy::literal_string_with_formatting_args,
        reason = "the golden XDC contains literal Tcl braces, not format args"
    )]
    fn golden_xdc_for_a_platform_child_pblock() {
        let result = FloorplanResult {
            device: "u280".to_string(),
            grid: (2, 3),
            regions: Map::from([("Worker_0".to_string(), "SLOT_X1Y1_TO_SLOT_X1Y1".to_string())]),
            routes: Vec::new(),
            slot_usage: Map::new(),
        };

        let expected = "\
create_pblock SLOT_X1Y1_TO_SLOT_X1Y1
set_property EXCLUDE_PLACEMENT 0 [get_pblocks SLOT_X1Y1_TO_SLOT_X1Y1]
set_property CONTAIN_ROUTING 0 [get_pblocks SLOT_X1Y1_TO_SLOT_X1Y1]
set_property IS_SOFT 1 [get_pblocks SLOT_X1Y1_TO_SLOT_X1Y1]
set_property PARENT pblock_dynamic_region [get_pblocks SLOT_X1Y1_TO_SLOT_X1Y1]
resize_pblock SLOT_X1Y1_TO_SLOT_X1Y1 -add {SLICE_X176Y240:SLICE_X196Y479}
resize_pblock SLOT_X1Y1_TO_SLOT_X1Y1 -add {DSP48E2_X25Y90:DSP48E2_X28Y185}
resize_pblock SLOT_X1Y1_TO_SLOT_X1Y1 -add {LAGUNA_X24Y120:LAGUNA_X27Y359}
resize_pblock SLOT_X1Y1_TO_SLOT_X1Y1 -add {RAMB18_X11Y96:RAMB18_X11Y191}
resize_pblock SLOT_X1Y1_TO_SLOT_X1Y1 -add {RAMB36_X11Y48:RAMB36_X11Y95}
resize_pblock SLOT_X1Y1_TO_SLOT_X1Y1 -add {URAM288_X4Y64:URAM288_X4Y127}
resize_pblock SLOT_X1Y1_TO_SLOT_X1Y1 -add {CLOCKREGION_X0Y4:CLOCKREGION_X5Y7}
resize_pblock SLOT_X1Y1_TO_SLOT_X1Y1 -remove CLOCKREGION_X0Y4:CLOCKREGION_X3Y7
set cells [get_cells -quiet -hierarchical -regexp -filter {NAME =~ \"^(.*/)?Worker_0(_fifo)?(/.*)?$\"}]
if {![llength $cells]} { error \"TAPA floorplan ERROR: expected cell `Worker_0` was not found\" }
add_cells_to_pblock SLOT_X1Y1_TO_SLOT_X1Y1 $cells
create_pblock TAPA_PARENT_CLIP_SLOT_X1Y1_TO_SLOT_X1Y1
set derived_ranges [get_property DERIVED_RANGES [get_pblocks SLOT_X1Y1_TO_SLOT_X1Y1]]
if {$derived_ranges ne \"\"} {
  resize_pblock TAPA_PARENT_CLIP_SLOT_X1Y1_TO_SLOT_X1Y1 -add $derived_ranges
}
set derived_ranges [get_property DERIVED_RANGES [get_pblocks pblock_dynamic_region]]
if {$derived_ranges ne \"\"} {
  resize_pblock TAPA_PARENT_CLIP_SLOT_X1Y1_TO_SLOT_X1Y1 -remove $derived_ranges
}
set derived_ranges [get_property DERIVED_RANGES [get_pblocks TAPA_PARENT_CLIP_SLOT_X1Y1_TO_SLOT_X1Y1]]
if {$derived_ranges ne \"\"} {
  resize_pblock SLOT_X1Y1_TO_SLOT_X1Y1 -remove $derived_ranges
}
delete_pblock -quiet TAPA_PARENT_CLIP_SLOT_X1Y1_TO_SLOT_X1Y1
";
        let device = select_device("u280").expect("u280");
        assert_eq!(emit_xdc(&result, &device), expected);
    }

    #[test]
    fn every_final_pblock_is_soft_with_ordered_properties() {
        let result = FloorplanResult {
            device: "u280".to_string(),
            grid: (2, 3),
            regions: Map::from([
                ("A_0".to_string(), "SLOT_X0Y0_TO_SLOT_X0Y0".to_string()),
                ("B_0".to_string(), "SLOT_X1Y1_TO_SLOT_X1Y1".to_string()),
            ]),
            routes: Vec::new(),
            slot_usage: Map::new(),
        };
        let device = select_device("u280").expect("u280");
        let xdc = emit_xdc(&result, &device);

        let pblock_count = xdc.matches("create_pblock SLOT_").count();
        assert_eq!(pblock_count, 2);
        assert_eq!(
            xdc.matches("set_property EXCLUDE_PLACEMENT 0 ").count(),
            pblock_count
        );
        assert_eq!(
            xdc.matches("set_property CONTAIN_ROUTING 0 ").count(),
            pblock_count
        );
        assert_eq!(xdc.matches("set_property IS_SOFT 1 ").count(), pblock_count);

        for pblock in ["SLOT_X0Y0_TO_SLOT_X0Y0", "SLOT_X1Y1_TO_SLOT_X1Y1"] {
            let section = pblock_section(&xdc, pblock);
            let exclude = section
                .find("set_property EXCLUDE_PLACEMENT 0 ")
                .expect("EXCLUDE_PLACEMENT property");
            let contain = section
                .find("set_property CONTAIN_ROUTING 0 ")
                .expect("CONTAIN_ROUTING property");
            let soft = section
                .find("set_property IS_SOFT 1 ")
                .expect("IS_SOFT property");
            let resize = section.find("resize_pblock ").expect("pblock resize");
            assert!(
                exclude < contain && contain < soft && soft < resize,
                "{section}"
            );
        }
    }

    #[test]
    fn cell_regex_disambiguates_prefix_indices_and_matches_fifo_suffix() {
        // `_1` must not swallow `_10`; a bracketed FIFO index sanitizes to the
        // underscore netlist name; and the `_fifo` suffix codegen adds to FIFO
        // and relay instances must match.
        let one = cell_name_regex("PEG_Xvec_1");
        let ten = cell_name_regex("PEG_Xvec_10");
        assert_eq!(one, "^(.*/)?PEG_Xvec_1(_fifo)?(/.*)?$");
        assert_eq!(ten, "^(.*/)?PEG_Xvec_10(_fifo)?(/.*)?$");
        // Under regex semantics `one` cannot match instance 10's hierarchy:
        // after `PEG_Xvec_1` it demands `_fifo`, `/`, or end — 10's next is `0`.
        assert!(
            !regex_matches(&one, "top/PEG_Xvec_10/u0"),
            "PEG_Xvec_1 pattern must not capture PEG_Xvec_10"
        );
        assert!(regex_matches(&one, "top/PEG_Xvec_1/u0"));
        assert!(regex_matches(&ten, "top/PEG_Xvec_10/u0"));

        // A bracketed FIFO name sanitizes to underscores, and its RTL instance
        // (`{sanitized}_fifo`) must match.
        let fifo = cell_name_regex("PE_inst[13]_Serpens");
        assert_eq!(fifo, "^(.*/)?PE_inst_13_Serpens(_fifo)?(/.*)?$");
        assert!(
            regex_matches(
                &fifo,
                "level0_i/ulp/Serpens/inst/PE_inst_13_Serpens_fifo/u0"
            ),
            "the relay/FIFO instance carries a _fifo suffix"
        );
        // And a task with no suffix still matches on its bare name.
        assert!(regex_matches(
            &cell_name_regex("Arbiter_Y_3"),
            "level0_i/ulp/Serpens/inst/Arbiter_Y_3/fsm"
        ));
        assert_eq!(
            cell_name_regex("Module1Func#1"),
            "^(.*/)?Module1Func_1(_fifo)?(/.*)?$",
        );
    }

    /// Mirror what our `^(.*/)?LITERAL(_fifo)?(/.*)?$` patterns mean: `LITERAL`,
    /// optionally with a `_fifo` suffix, is a complete `/`-delimited path
    /// component. Vivado runs the real regex at implementation time; this only
    /// backs the assertions above.
    fn regex_matches(pattern: &str, name: &str) -> bool {
        let literal = pattern
            .strip_prefix("^(.*/)?")
            .and_then(|p| p.strip_suffix("(_fifo)?(/.*)?$"))
            .expect("pattern shape")
            .replace('\\', "");
        let with_fifo = format!("{literal}_fifo");
        name.split('/')
            .any(|component| component == literal || component == with_fifo)
    }

    #[test]
    fn multi_slot_region_unions_ranges() {
        let result = FloorplanResult {
            device: "u280".to_string(),
            grid: (2, 3),
            regions: Map::from([("T".to_string(), "SLOT_X0Y1_TO_SLOT_X1Y1".to_string())]),
            routes: Vec::new(),
            slot_usage: Map::new(),
        };
        let device = select_device("u280").expect("u280");
        let xdc = emit_xdc(&result, &device);
        assert!(
            xdc.contains(
                "resize_pblock TAPA_SLOT_UNION_SLOT_X0Y1_TO_SLOT_X1Y1_0 -remove \
                 CLOCKREGION_X4Y4:CLOCKREGION_X7Y7"
            ),
            "left slot must subtract only the right half:\n{xdc}"
        );
        assert!(
            xdc.contains(
                "resize_pblock TAPA_SLOT_UNION_SLOT_X0Y1_TO_SLOT_X1Y1_1 -remove \
                 CLOCKREGION_X0Y4:CLOCKREGION_X3Y7"
            ),
            "right slot must subtract only the left half:\n{xdc}"
        );
        assert_eq!(
            xdc.matches("set slot_ranges [get_property DERIVED_RANGES")
                .count(),
            2,
            "each atomic slot must be derived independently before union"
        );
        assert_eq!(
            xdc.matches("resize_pblock SLOT_X0Y1_TO_SLOT_X1Y1 -add $slot_ranges")
                .count(),
            2,
        );
    }

    #[test]
    fn crossing_pipeline_children_are_constrained_stage_by_stage() {
        let result = FloorplanResult {
            device: "u280".to_string(),
            grid: (2, 3),
            regions: Map::from([
                (
                    "producer_0".to_string(),
                    "SLOT_X0Y0_TO_SLOT_X0Y0".to_string(),
                ),
                (
                    "consumer_0".to_string(),
                    "SLOT_X1Y1_TO_SLOT_X1Y1".to_string(),
                ),
                // This old alias must not pin the whole split pipeline.
                ("fifo_0".to_string(), "SLOT_X1Y1_TO_SLOT_X1Y1".to_string()),
            ]),
            routes: vec![PipelineRoute {
                channel: RoutedChannel::Stream {
                    fifo: "fifo_0".to_string(),
                },
                route: vec![
                    "SLOT_X0Y0".to_string(),
                    "SLOT_X1Y0".to_string(),
                    "SLOT_X1Y1".to_string(),
                ],
                scheme: PipelineScheme::Double,
                reg_regions: vec!["SLOT_X0Y0".to_string(), "SLOT_X1Y0".to_string()],
            }],
            slot_usage: Map::new(),
        };
        let device = select_device("u280").expect("u280");
        let xdc = emit_xdc(&result, &device);

        let source = pblock_section(&xdc, "SLOT_X0Y0_TO_SLOT_X0Y0");
        assert!(
            source.contains(&pipeline_head_regex("fifo_0_fifo")),
            "{source}"
        );
        assert!(
            source.contains(&pipeline_body_regex("fifo_0_fifo", 0)),
            "{source}"
        );

        let middle = pblock_section(&xdc, "SLOT_X1Y0_TO_SLOT_X1Y0");
        assert!(
            middle.contains(&pipeline_body_regex("fifo_0_fifo", 1)),
            "{middle}"
        );

        let destination = pblock_section(&xdc, "SLOT_X1Y1_TO_SLOT_X1Y1");
        assert!(
            destination.contains(&pipeline_tail_regex("fifo_0_fifo")),
            "{destination}"
        );
        assert!(destination.contains(&cell_name_regex("consumer_0")));
        assert!(
            !xdc.contains(&cell_name_regex("fifo_0")),
            "the crossing's monolithic parent must not receive a placement constraint:\n{xdc}"
        );
        for description in [
            "fifo_0 Head",
            "fifo_0 Body 0",
            "fifo_0 Body 1",
            "fifo_0 Tail",
        ] {
            assert_missing_cell_is_fatal(&xdc, description);
        }
    }

    #[test]
    fn zero_body_crossing_still_constrains_head_and_tail() {
        let result = FloorplanResult {
            device: "u280".to_string(),
            grid: (2, 3),
            regions: Map::from([(
                "fifo_adjacent".to_string(),
                "SLOT_X0Y1_TO_SLOT_X0Y1".to_string(),
            )]),
            routes: vec![PipelineRoute {
                channel: RoutedChannel::Stream {
                    fifo: "fifo_adjacent".to_string(),
                },
                route: vec!["SLOT_X0Y0".to_string(), "SLOT_X0Y1".to_string()],
                scheme: PipelineScheme::Single,
                reg_regions: Vec::new(),
            }],
            slot_usage: Map::new(),
        };
        let xdc = emit_xdc(&result, &select_device("u280").expect("u280"));

        assert!(
            xdc.contains(&pipeline_head_regex("fifo_adjacent_fifo")),
            "{xdc}"
        );
        assert!(
            xdc.contains(&pipeline_tail_regex("fifo_adjacent_fifo")),
            "{xdc}"
        );
        assert!(!xdc.contains("TAPA_HS_BODY\\["), "{xdc}");
    }

    #[test]
    fn axi_channel_pipeline_uses_its_typed_hierarchy_and_route_direction() {
        let endpoint = AxiEndpoint {
            instance: "Reader_0".to_string(),
            port: "mem".to_string(),
            top_port: "data".to_string(),
        };
        let pipeline = axi_pipeline_instance_name(&endpoint, AxiChannel::ReadData);
        let description = format!(
            "{}.{} {}",
            endpoint.instance,
            endpoint.port,
            AxiChannel::ReadData.rtl_name(),
        );
        let result = FloorplanResult {
            device: "u280".to_string(),
            grid: (2, 3),
            regions: Map::from([(
                endpoint.instance.clone(),
                "SLOT_X1Y0_TO_SLOT_X1Y0".to_string(),
            )]),
            routes: vec![PipelineRoute {
                channel: RoutedChannel::Axi {
                    endpoint,
                    bank: MemoryBank {
                        kind: MemoryKind::Hbm,
                        index: 0,
                    },
                    channel: AxiChannel::ReadData,
                },
                // Read data runs from the bank to the child.
                route: vec!["SLOT_X0Y0".to_string(), "SLOT_X1Y0".to_string()],
                scheme: PipelineScheme::Double,
                reg_regions: vec!["SLOT_X0Y0".to_string(), "SLOT_X1Y0".to_string()],
            }],
            slot_usage: Map::new(),
        };
        let xdc = emit_xdc(&result, &select_device("u280").expect("u280"));

        let bank = pblock_section(&xdc, "SLOT_X0Y0_TO_SLOT_X0Y0");
        assert!(bank.contains(&pipeline_head_regex(&pipeline)), "{bank}");
        assert!(bank.contains(&pipeline_body_regex(&pipeline, 0)), "{bank}");
        let child = pblock_section(&xdc, "SLOT_X1Y0_TO_SLOT_X1Y0");
        assert!(
            child.contains(&pipeline_body_regex(&pipeline, 1)),
            "{child}"
        );
        assert!(child.contains(&pipeline_tail_regex(&pipeline)), "{child}");

        for stage in ["Head", "Body 0", "Body 1", "Tail"] {
            assert_missing_cell_is_fatal(&xdc, &format!("{description} {stage}"));
        }
    }

    #[test]
    fn distributed_controllers_and_control_stages_are_constrained() {
        let instance = "worker#0";
        let global = global_controller_instance_name().to_string();
        let local = local_controller_instance_name(instance);
        let pipeline = control_pipeline_instance_name(instance, ControlChannel::Launch);
        let result = FloorplanResult {
            device: "u280".to_string(),
            grid: (2, 3),
            regions: Map::from([
                (global.clone(), "SLOT_X0Y0_TO_SLOT_X0Y0".to_string()),
                (
                    "control_s_axi_U".to_string(),
                    "SLOT_X0Y0_TO_SLOT_X0Y0".to_string(),
                ),
                (instance.to_string(), "SLOT_X0Y2_TO_SLOT_X0Y2".to_string()),
                (local.clone(), "SLOT_X0Y2_TO_SLOT_X0Y2".to_string()),
            ]),
            routes: vec![PipelineRoute {
                channel: RoutedChannel::Control {
                    instance: instance.to_string(),
                    channel: ControlChannel::Launch,
                },
                route: vec![
                    "SLOT_X0Y0".to_string(),
                    "SLOT_X0Y1".to_string(),
                    "SLOT_X0Y2".to_string(),
                ],
                scheme: PipelineScheme::Single,
                reg_regions: vec!["SLOT_X0Y1".to_string()],
            }],
            slot_usage: Map::new(),
        };
        let xdc = emit_xdc(&result, &select_device("u280").expect("u280"));

        let source = pblock_section(&xdc, "SLOT_X0Y0_TO_SLOT_X0Y0");
        assert!(source.contains(&cell_name_regex(&global)), "{source}");
        assert!(
            source.contains(&cell_name_regex("control_s_axi_U")),
            "{source}"
        );
        assert!(source.contains(&pipeline_head_regex(&pipeline)), "{source}");
        let body = pblock_section(&xdc, "SLOT_X0Y1_TO_SLOT_X0Y1");
        assert!(body.contains(&pipeline_body_regex(&pipeline, 0)), "{body}");
        let destination = pblock_section(&xdc, "SLOT_X0Y2_TO_SLOT_X0Y2");
        assert!(
            destination.contains(&cell_name_regex(&local)),
            "{destination}"
        );
        assert!(
            destination.contains(&pipeline_tail_regex(&pipeline)),
            "{destination}"
        );

        for expected in [
            global.as_str(),
            "control_s_axi_U",
            local.as_str(),
            "worker#0 launch Head",
            "worker#0 launch Body 0",
            "worker#0 launch Tail",
        ] {
            assert_missing_cell_is_fatal(&xdc, expected);
        }
    }

    #[test]
    fn missing_cell_diagnostic_escapes_tcl_substitutions() {
        let result = FloorplanResult {
            device: "u280".to_string(),
            grid: (2, 3),
            regions: Map::from([(
                "fifo[0]$quoted\"".to_string(),
                "SLOT_X0Y0_TO_SLOT_X0Y0".to_string(),
            )]),
            routes: Vec::new(),
            slot_usage: Map::new(),
        };
        let xdc = emit_xdc(&result, &select_device("u280").expect("u280"));

        assert_missing_cell_is_fatal(&xdc, "fifo[0]$quoted\"");
        assert!(
            xdc.contains("expected cell `fifo\\[0]\\$quoted\\\"`"),
            "{xdc}"
        );
    }

    fn assert_missing_cell_is_fatal(xdc: &str, description: &str) {
        let diagnostic = format!(
            "if {{![llength $cells]}} {{ error \"TAPA floorplan ERROR: expected cell `{}` was not found\" }}",
            tcl_double_quote_escape(description),
        );
        assert!(
            xdc.contains(&diagnostic),
            "missing `{diagnostic}` in:\n{xdc}"
        );
        assert!(
            xdc.contains("get_cells -quiet -hierarchical -regexp"),
            "{xdc}"
        );
        assert!(!xdc.contains("TAPA floorplan WARNING"), "{xdc}");
    }

    fn pblock_section<'a>(xdc: &'a str, pblock: &str) -> &'a str {
        let marker = format!("create_pblock {pblock}\n");
        let start = xdc.find(&marker).expect("pblock exists");
        let rest = &xdc[start + marker.len()..];
        let end = rest.find("\ncreate_pblock ").unwrap_or(rest.len());
        &rest[..end]
    }
}
