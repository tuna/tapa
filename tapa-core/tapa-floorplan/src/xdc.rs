//! Pblock XDC emission: turn a [`FloorplanResult`] plus its [`Device`] into the
//! `create_pblock`/`resize_pblock`/`add_cells_to_pblock` constraints Vivado
//! applies during implementation.
//!
//! Cells are matched with hierarchical wildcards so they still resolve once
//! `v++` places the kernel below a platform prefix and synthesis flattens names.

use std::collections::{BTreeMap, BTreeSet};

use tapa_ir::port::sanitize_array_name;
use tapa_ir::{CrossingKind, FloorplanResult};

use crate::device::model::{Coor, Device};

/// Render the floorplan as XDC pblock constraints, terminated by a newline.
#[must_use]
pub fn emit_xdc(result: &FloorplanResult, device: &Device) -> String {
    // A crossing FIFO is split into independently constrained Head/Body/Tail
    // hierarchy below. Constraining its monolithic parent as well would force
    // every stage back into the FIFO vertex's old placement pblock.
    let crossing_streams: BTreeSet<&str> = result
        .crossings
        .iter()
        .filter(|crossing| crossing.kind == CrossingKind::Stream)
        .map(|crossing| crossing.link.as_str())
        .collect();

    // Group cell matches by physical region so each pblock is created once.
    let mut by_region: BTreeMap<String, Vec<CellMatch>> = BTreeMap::new();
    for (instance, region) in &result.regions {
        if crossing_streams.contains(instance.as_str()) {
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

    for crossing in &result.crossings {
        if crossing.kind != CrossingKind::Stream {
            continue;
        }
        let (Some(head_region), Some(tail_region)) =
            (crossing.route.first(), crossing.route.last())
        else {
            continue;
        };

        by_region
            .entry(canonical_pblock_name(head_region))
            .or_default()
            .push(CellMatch {
                pattern: pipeline_head_regex(&crossing.link),
                description: format!("{} Head", crossing.link),
            });
        for (index, region) in crossing.reg_regions.iter().enumerate() {
            by_region
                .entry(canonical_pblock_name(region))
                .or_default()
                .push(CellMatch {
                    pattern: pipeline_body_regex(&crossing.link, index),
                    description: format!("{} Body {index}", crossing.link),
                });
        }
        by_region
            .entry(canonical_pblock_name(tail_region))
            .or_default()
            .push(CellMatch {
                pattern: pipeline_tail_regex(&crossing.link),
                description: format!("{} Tail", crossing.link),
            });
    }

    let mut lines: Vec<String> = Vec::new();
    for (region, matches) in &by_region {
        let ranges = region_pblock_ranges(device, region);
        lines.push(format!("create_pblock {region}"));
        lines.push(format!(
            "resize_pblock {region} -add {{{}}}",
            ranges.join(" ")
        ));
        for cell_match in matches {
            // `get_cells` errors on an empty result, so guard each assignment:
            // a stray unmatched instance (e.g. one synthesis optimized away)
            // logs a warning instead of aborting the whole implementation.
            lines.push(format!(
                "set cells [get_cells -hierarchical -regexp -filter {{NAME =~ \"{}\"}}]",
                cell_match.pattern
            ));
            lines.push(format!(
                "if {{[llength $cells]}} {{ add_cells_to_pblock {region} $cells }} \
                 else {{ puts \"TAPA floorplan WARNING: no cells matched {}\" }}",
                cell_match.description
            ));
        }
    }

    let mut text = lines.join("\n");
    text.push('\n');
    text
}

struct CellMatch {
    pattern: String,
    description: String,
}

/// A Vivado `-regexp` `NAME` pattern matching `instance`'s RTL cell as a
/// *complete* hierarchy path component: preceded by the netlist root or a `/`,
/// and followed by the end of the name or a `/` (its descendant cells).
///
/// Two transforms bridge the graph name to the netlist name codegen emits:
///   * `sanitize_array_name` collapses a bracketed index (`PE_inst[0]_Serpens`)
///     to the underscore form the Verilog instance uses (`PE_inst_0_Serpens`);
///   * an optional `_fifo` suffix matches FIFO and handshake-pipeline instances,
///     which codegen names `{sanitized}_fifo`, while leaf tasks carry no suffix.
///
/// Anchoring keeps `PEG_Xvec_1` from also capturing `PEG_Xvec_10..19`.
fn cell_name_regex(instance: &str) -> String {
    format!(
        "^(.*/)?{}(_fifo)?(/.*)?$",
        regex_escape(&sanitize_array_name(instance))
    )
}

/// Match the two source-slot Head cells below a generated stream pipeline.
/// The gate is combinational, while `TAPA_HS_HEAD` owns the actual ready,
/// valid, and data registers. Keeping both in one match also survives Vivado
/// retaining the gate as a separate hierarchy cell.
fn pipeline_head_regex(link: &str) -> String {
    let fifo = format!("{}_fifo", sanitize_array_name(link));
    format!("^(.*/)?{}/TAPA_HS_HEAD(_GATE)?(/.*)?$", regex_escape(&fifo))
}

/// Match one generated Body register and all of its descendants.
///
/// Vivado may render a named generate scope separator as either `/` or `.`, so
/// the middle `.*` deliberately accepts both while the escaped index and
/// complete child name keep the match exact.
fn pipeline_body_regex(link: &str, index: usize) -> String {
    let fifo = format!("{}_fifo", sanitize_array_name(link));
    format!(
        "^(.*/)?{}/TAPA_HS_BODY\\[{index}\\].*TAPA_HS_BODY_REG(/.*)?$",
        regex_escape(&fifo)
    )
}

/// Match the destination-slot Tail FIFO and every cell below it.
fn pipeline_tail_regex(link: &str) -> String {
    let fifo = format!("{}_fifo", sanitize_array_name(link));
    format!("^(.*/)?{}/TAPA_HS_TAIL(/.*)?$", regex_escape(&fifo))
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

/// The union of pblock ranges of every atomic slot a region covers.
fn region_pblock_ranges(device: &Device, region: &str) -> Vec<String> {
    let Some(coor) = parse_region_or_slot(region) else {
        return Vec::new();
    };
    let mut ranges = Vec::new();
    for (x, y) in coor.all_slot_coors() {
        if let Some(slot) = device.slot(x, y) {
            ranges.extend(slot.pblock_ranges.iter().cloned());
        }
    }
    ranges
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
    use tapa_ir::{Area, Crossing, PipelineScheme};

    #[test]
    #[allow(
        clippy::literal_string_with_formatting_args,
        reason = "the golden XDC contains literal Tcl braces, not format args"
    )]
    fn golden_xdc_for_a_colocated_design() {
        let result = FloorplanResult {
            device: "u280".to_string(),
            grid: (2, 3),
            regions: Map::from([
                ("A_0".to_string(), "SLOT_X0Y0_TO_SLOT_X0Y0".to_string()),
                ("B_0".to_string(), "SLOT_X0Y0_TO_SLOT_X0Y0".to_string()),
                (
                    "fifo_VecAdd".to_string(),
                    "SLOT_X0Y0_TO_SLOT_X0Y0".to_string(),
                ),
            ]),
            crossings: Vec::new(),
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
        let device = select_device("u280").expect("u280");

        let expected = "\
create_pblock SLOT_X0Y0_TO_SLOT_X0Y0
resize_pblock SLOT_X0Y0_TO_SLOT_X0Y0 -add {CLOCKREGION_X0Y0:CLOCKREGION_X3Y3}
set cells [get_cells -hierarchical -regexp -filter {NAME =~ \"^(.*/)?A_0(_fifo)?(/.*)?$\"}]
if {[llength $cells]} { add_cells_to_pblock SLOT_X0Y0_TO_SLOT_X0Y0 $cells } else { puts \"TAPA floorplan WARNING: no cells matched A_0\" }
set cells [get_cells -hierarchical -regexp -filter {NAME =~ \"^(.*/)?B_0(_fifo)?(/.*)?$\"}]
if {[llength $cells]} { add_cells_to_pblock SLOT_X0Y0_TO_SLOT_X0Y0 $cells } else { puts \"TAPA floorplan WARNING: no cells matched B_0\" }
set cells [get_cells -hierarchical -regexp -filter {NAME =~ \"^(.*/)?fifo_VecAdd(_fifo)?(/.*)?$\"}]
if {[llength $cells]} { add_cells_to_pblock SLOT_X0Y0_TO_SLOT_X0Y0 $cells } else { puts \"TAPA floorplan WARNING: no cells matched fifo_VecAdd\" }
";
        assert_eq!(emit_xdc(&result, &device), expected);
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
        // A region spanning column 0's bottom two rows unions both pblocks.
        let result = FloorplanResult {
            device: "u280".to_string(),
            grid: (2, 3),
            regions: Map::from([("T".to_string(), "SLOT_X0Y0_TO_SLOT_X0Y1".to_string())]),
            crossings: Vec::new(),
            slot_usage: Map::new(),
        };
        let device = select_device("u280").expect("u280");
        let xdc = emit_xdc(&result, &device);
        assert!(
            xdc.contains("CLOCKREGION_X0Y0:CLOCKREGION_X3Y3"),
            "row 0 range"
        );
        assert!(
            xdc.contains("CLOCKREGION_X0Y4:CLOCKREGION_X3Y7"),
            "row 1 range"
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
            crossings: vec![Crossing {
                kind: CrossingKind::Stream,
                link: "fifo_0".to_string(),
                route: vec![
                    "SLOT_X0Y0".to_string(),
                    "SLOT_X1Y0".to_string(),
                    "SLOT_X1Y1".to_string(),
                ],
                level: 2,
                scheme: PipelineScheme::Double,
                reg_regions: vec!["SLOT_X0Y0".to_string(), "SLOT_X1Y0".to_string()],
            }],
            slot_usage: Map::new(),
        };
        let device = select_device("u280").expect("u280");
        let xdc = emit_xdc(&result, &device);

        let source = pblock_section(&xdc, "SLOT_X0Y0_TO_SLOT_X0Y0");
        assert!(source.contains(&pipeline_head_regex("fifo_0")), "{source}");
        assert!(
            source.contains(&pipeline_body_regex("fifo_0", 0)),
            "{source}"
        );

        let middle = pblock_section(&xdc, "SLOT_X1Y0_TO_SLOT_X1Y0");
        assert!(
            middle.contains(&pipeline_body_regex("fifo_0", 1)),
            "{middle}"
        );

        let destination = pblock_section(&xdc, "SLOT_X1Y1_TO_SLOT_X1Y1");
        assert!(
            destination.contains(&pipeline_tail_regex("fifo_0")),
            "{destination}"
        );
        assert!(destination.contains(&cell_name_regex("consumer_0")));
        assert!(
            !xdc.contains(&cell_name_regex("fifo_0")),
            "the crossing's monolithic parent must not receive a placement constraint:\n{xdc}"
        );
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
            crossings: vec![Crossing {
                kind: CrossingKind::Stream,
                link: "fifo_adjacent".to_string(),
                route: vec!["SLOT_X0Y0".to_string(), "SLOT_X0Y1".to_string()],
                level: 0,
                scheme: PipelineScheme::Single,
                reg_regions: Vec::new(),
            }],
            slot_usage: Map::new(),
        };
        let xdc = emit_xdc(&result, &select_device("u280").expect("u280"));

        assert!(xdc.contains(&pipeline_head_regex("fifo_adjacent")), "{xdc}");
        assert!(xdc.contains(&pipeline_tail_regex("fifo_adjacent")), "{xdc}");
        assert!(!xdc.contains("TAPA_HS_BODY\\["), "{xdc}");
    }

    fn pblock_section<'a>(xdc: &'a str, pblock: &str) -> &'a str {
        let marker = format!("create_pblock {pblock}\n");
        let start = xdc.find(&marker).expect("pblock exists");
        let rest = &xdc[start + marker.len()..];
        let end = rest.find("\ncreate_pblock ").unwrap_or(rest.len());
        &rest[..end]
    }
}
