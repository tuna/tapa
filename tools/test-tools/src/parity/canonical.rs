//! Canonical form of Vitis-HLS-generated verilog, for parity hashing.
//!
//! Vitis HLS bakes source line numbers into the names it generates: the
//! module/file names of pipelined loops (`Add_Pipeline_VITIS_LOOP_38_1.v`)
//! and the signal names of line-attributed operations (`icmp_ln55`,
//! `or_ln49_1`). Two syntheses of the *same* design whose C++ differs
//! only in line layout — e.g. formatted vs raw flattened input, or the
//! campaign's mirrored tree with its `#line` re-snaps — therefore
//! produce RTL that differs in exactly those tokens and nothing else.
//! That was established empirically by diffing two such generations
//! across all ten parity apps: every differing identifier belongs to
//! one of the two families rewritten below, and the only other byte
//! difference is the order of independent statements, which HLS keys on
//! those same line-bearing names.
//!
//! Canonicalization rewrites both families to per-file ranked
//! placeholders and sorts the lines of the result, so semantically
//! identical RTL hashes equal while content drift (literals, port sets,
//! module sets, module bodies) still trips the gate. Residual blind
//! spots, stated honestly: a change that only shifts line numbers while
//! leaving loop structure intact canonicalizes equal by construction —
//! that is precisely the benign variance being removed — and, because
//! lines are sorted, so does a change that only relocates a
//! byte-identical statement between existing procedural blocks.
//!
//! The rewritten-C++ side of parity stays RAW: it is our own producer's
//! output and remains byte-accountable.

use std::collections::BTreeMap;

/// Prefix of HLS loop names: `VITIS_LOOP_<line>[_<loop id>]` — e.g.
/// `Add_Add_Pipeline_VITIS_LOOP_39_1`. The line component varies with
/// source layout; the trailing loop id is structural and kept verbatim,
/// so removing a loop (which renumbers the survivors' ids) still
/// changes the canonical form.
const LOOP_TAG: &str = "VITIS_LOOP_";

/// Infix of HLS operation names: `<op>_ln<line>[_<occurrence>]` — e.g.
/// `icmp_ln38`, `or_ln49_1` — embedded inside larger identifiers such as
/// `ap_phi_mux_phi_ln50_phi_fu_100_p4`. Both the line and the
/// same-line occurrence suffix vary with source layout (two ops the
/// formatter splits onto separate lines sit on one line in raw input),
/// so both are folded into the rank.
const LN_TAG: &str = "_ln";

/// Placeholder separator for ranks. `~` cannot appear in HLS-generated
/// identifiers, so placeholders never collide with real tokens.
const RANK_SEP: char = '~';

/// Canonicalize verilog text or a file name: rewrite every
/// `VITIS_LOOP_<line>` to `VITIS_LOOP_~<rank>` and every
/// `<prefix>_ln<line>[_<occurrence>]` to `<prefix>_ln~<rank>`. Ranks
/// number the distinct original tokens of each family — per tag for
/// `VITIS_LOOP_`, per identifier prefix for `_ln` — in ascending
/// (line, occurrence) order: numeric, not lexicographic, and
/// independent of the order statements appear in the file.
pub fn canonical_verilog(text: &str) -> String {
    let loops = rewrite_vitis_loops(text);
    rewrite_ln_units(&loops)
}

/// Canonical form of one verilog file's contents for hashing: token
/// normalization, then the lines sorted, so the statement order — which
/// HLS derives from the line-bearing names — does not count either.
/// Line multiplicity is preserved: inserting or deleting a statement
/// still changes the canonical bytes.
pub fn canonical_verilog_contents(text: &str) -> String {
    let normalized = canonical_verilog(text);
    let mut lines: Vec<&str> = normalized.split('\n').collect();
    lines.sort_unstable();
    lines.join("\n")
}

/// One `<prefix>_ln<line>[_<occurrence>]` occurrence: byte range of the
/// `_ln...` unit within the original text, the family it belongs to,
/// and its (line, occurrence) rank key.
struct LnUnit {
    unit_start: usize,
    unit_end: usize,
    prefix: String,
    line: u64,
    occurrence: u64,
}

fn rewrite_vitis_loops(text: &str) -> String {
    let mut lines = Vec::new();
    for_each_number_after(text, LOOP_TAG, |_, line, _| {
        if !lines.contains(&line) {
            lines.push(line);
        }
    });
    lines.sort_unstable();
    let rank_of = |line: u64| 1 + lines.binary_search(&line).expect("ranked line is present");
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for_each_number_after(text, LOOP_TAG, |tag_start, line, num_end| {
        out.push_str(&text[cursor..tag_start]);
        out.push_str(LOOP_TAG);
        out.push(RANK_SEP);
        out.push_str(&rank_of(line).to_string());
        cursor = num_end;
    });
    out.push_str(&text[cursor..]);
    out
}

fn rewrite_ln_units(text: &str) -> String {
    let units = scan_ln_units(text);
    // Rank (line, occurrence) keys per family, ascending; distinct
    // originals can never tie (the scanned unit *is* family + line +
    // occurrence).
    let mut families: BTreeMap<String, Vec<(u64, u64)>> = BTreeMap::new();
    for unit in &units {
        let keys = families.entry(unit.prefix.clone()).or_default();
        if !keys.contains(&(unit.line, unit.occurrence)) {
            keys.push((unit.line, unit.occurrence));
        }
    }
    for keys in families.values_mut() {
        keys.sort_unstable();
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for unit in &units {
        let keys = &families[&unit.prefix];
        let rank = 1 + keys
            .binary_search(&(unit.line, unit.occurrence))
            .expect("scanned unit is ranked");
        out.push_str(&text[cursor..unit.unit_start]);
        out.push_str(LN_TAG);
        out.push(RANK_SEP);
        out.push_str(&rank.to_string());
        cursor = unit.unit_end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Scan for `tag` immediately followed by an ASCII-digit run, invoking
/// `hit(tag_start, number, digits_end)` for every occurrence.
fn for_each_number_after<F: FnMut(usize, u64, usize)>(text: &str, tag: &str, mut hit: F) {
    let mut from = 0;
    while let Some(found) = text[from..].find(tag) {
        let tag_start = from + found;
        let num_start = tag_start + tag.len();
        if let Some((number, len)) = leading_number(&text[num_start..]) {
            hit(tag_start, number, num_start + len);
            from = num_start + len;
        } else {
            from = num_start;
        }
    }
}

/// The leading ASCII-digit run of `text`, as a number plus its length.
fn leading_number(text: &str) -> Option<(u64, usize)> {
    let len = text.bytes().take_while(u8::is_ascii_digit).count();
    if len == 0 {
        return None;
    }
    let number = text[..len].parse().expect("digit run fits u64");
    Some((number, len))
}

/// Every `_ln` unit in `text`, leftmost per identifier run of
/// `[A-Za-z0-9_]` characters: the first `_ln<digits>[_<digits>]` inside
/// the run whose prefix is non-empty. The prefix (everything from the
/// run's start up to `_ln`) is the family key and is left verbatim in
/// the output.
fn scan_ln_units(text: &str) -> Vec<LnUnit> {
    let bytes = text.as_bytes();
    let is_id = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut units = Vec::new();
    let mut run_start = None;
    let mut index = 0;
    while index <= bytes.len() {
        let in_run = index < bytes.len() && is_id(bytes[index]);
        if in_run {
            if run_start.is_none() {
                run_start = Some(index);
            }
            index += 1;
            continue;
        }
        if let Some(start) = run_start.take() {
            if let Some(unit) = first_ln_unit(text, start, index) {
                units.push(unit);
            }
        }
        index += 1;
    }
    units
}

/// The first `_ln<digits>[_<digits>]` unit in `text[start..end]` with a
/// non-empty prefix, if any.
fn first_ln_unit(text: &str, start: usize, end: usize) -> Option<LnUnit> {
    let mut from = start;
    while let Some(found) = text[from..end].find(LN_TAG) {
        let unit_start = from + found;
        if unit_start == start {
            return None; // empty prefix: not an `<op>_ln` name
        }
        let num_start = unit_start + LN_TAG.len();
        let Some((line, len)) = leading_number(&text[num_start..end]) else {
            from = num_start;
            continue;
        };
        let after_line = num_start + len;
        // Optional same-line occurrence suffix `_<digits>`.
        let (occurrence, unit_end) = if text[after_line..end].starts_with('_')
            && leading_number(&text[after_line + 1..end]).is_some_and(|(_, n)| n > 0)
        {
            let occ_start = after_line + 1;
            let (occ, occ_len) = leading_number(&text[occ_start..end]).expect("checked above");
            (occ, occ_start + occ_len)
        } else {
            (0, after_line)
        };
        return Some(LnUnit {
            unit_start,
            unit_end,
            prefix: text[start..unit_start].to_string(),
            line,
            occurrence,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ranks follow numeric line order, never lexicographic, and equal
    // loop names share a rank; trailing loop ids stay verbatim so two
    // loops of one task never collide.
    #[test]
    fn vitis_loop_ranks_are_numeric_and_ids_survive() {
        assert_eq!(
            canonical_verilog("Add_Add_Pipeline_VITIS_LOOP_39_1.v"),
            "Add_Add_Pipeline_VITIS_LOOP_~1_1.v"
        );
        assert_eq!(
            canonical_verilog("VITIS_LOOP_100_1 VITIS_LOOP_99_2 VITIS_LOOP_100_1"),
            "VITIS_LOOP_~2_1 VITIS_LOOP_~1_2 VITIS_LOOP_~2_1"
        );
    }

    // Operation names rank per prefix family; the same-line occurrence
    // suffix folds into the rank (raw input puts two ops on one line
    // that formatted input splits); structural suffixes like `_fu_128`
    // are not part of the unit.
    #[test]
    fn ln_units_rank_per_prefix_and_fold_occurrences() {
        assert_eq!(
            canonical_verilog("or_ln49_1_fu_190_p2 or_ln49_fu_172_p2 icmp_ln30"),
            "or_ln~2_fu_190_p2 or_ln~1_fu_172_p2 icmp_ln~1"
        );
        assert_eq!(
            canonical_verilog("icmp_ln99 icmp_ln100 icmp_ln98"),
            "icmp_ln~2 icmp_ln~3 icmp_ln~1"
        );
        assert_eq!(
            canonical_verilog("ap_phi_mux_phi_ln50_phi_fu_100_p4"),
            "ap_phi_mux_phi_ln~1_phi_fu_100_p4"
        );
    }

    // Text without the families is untouched, including `_ln` not
    // followed by digits.
    #[test]
    fn unrelated_text_is_untouched() {
        let text = "wire [31:0] a; // _lnfoo VITIS_LOOP\nassign a = data_q_s_din;";
        assert_eq!(canonical_verilog(text), text);
    }

    // Statement order is canonical away, but content is not: a changed
    // constant flips the canonical bytes.
    #[test]
    fn contents_hash_ignores_statement_order_not_content() {
        let a = canonical_verilog_contents("assign x = icmp_ln55;\nassign y = 1'b1;\n");
        let b = canonical_verilog_contents("assign y = 1'b1;\nassign x = icmp_ln56;\n");
        let sabotaged = canonical_verilog_contents("assign x = icmp_ln55;\nassign y = 1'b0;\n");
        assert_eq!(a, b);
        assert_ne!(a, sabotaged);
    }
}
