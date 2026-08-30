//! Canonical form of Vitis-HLS-generated verilog, for parity hashing.
//!
//! Vitis HLS bakes source line numbers into the names it generates: the
//! module/file names of pipelined loops (`Add_Pipeline_VITIS_LOOP_38_1.v`)
//! and the signal names of line-attributed operations (`icmp_ln55`,
//! `or_ln49_1`). Two syntheses of the *same* design whose C++ differs
//! only in line layout — e.g. the campaign's mirrored tree with its
//! `#line` re-snaps versus the flattened blob — produce RTL that
//! differs in exactly those tokens and nothing else (established
//! empirically by diffing such generations across the parity apps);
//! the only other byte difference is the order of independent
//! statements, which HLS keys on those same line-bearing names.
//!
//! The two layouts do not even agree on the *order* of the line
//! numbers: a `#line` re-snap maps one construct back to user line 81
//! while the flattened blob places it at line 96, behind a construct
//! that stays at line 94 in both. So canonicalization ranks nothing by
//! numeric line — that was the previous design and it drifted. Instead
//! it erases the line-number component of both families with a fixed
//! marker, keeps every structural suffix verbatim (loop ids,
//! `_fu_N`/`_reg_N` allocation numbers, `_cast`, …), and sorts the
//! lines of the result, so semantically identical RTL hashes equal
//! while content drift (literals, port sets, module sets, module
//! bodies, loop counts) still trips the gate.
//!
//! Erasing identity can also make two distinct generated *files* share
//! one normalized path — two pipelined loops of one task with the same
//! loop id at different source lines. Such artifacts are keyed as a
//! sorted multiset (`path#<ordinal>`, ordinals by sorted canonical
//! content hash) by [`keyed_multiset`], never silently merged.
//!
//! Residual blind spots, stated honestly:
//! - A change that only shifts line numbers while leaving loop
//!   structure intact canonicalizes equal by construction — that is
//!   precisely the benign variance being removed.
//! - The same-line occurrence suffix is erased with the line, so
//!   regrouping operations across source lines (two ops sharing one
//!   line split onto two, or the reverse) is invisible.
//! - Because lines are sorted, relocating a byte-identical statement
//!   between existing procedural blocks is invisible.
//! - Two loop modules differing only in source line normalize to one
//!   path; the multiset preserves their count and contents but not
//!   their pairing to physical files, and a parent module's
//!   instantiations of both normalize to one module name, so which
//!   instance binds to which file is not distinguished.
//! - `_fu_N`/`_reg_N` allocation numbers are kept verbatim and assumed
//!   stable across equivalent syntheses (they were, on every corpus
//!   examined); allocation-only churn would register as drift.
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
/// `icmp_ln38`, `or_ln49_1` — embedded inside larger identifiers such
/// as `ap_phi_mux_phi_ln50_phi_fu_100_p4`. Both the line and the
/// same-line occurrence vary with source layout, so both are erased;
/// structural suffixes around them (`_fu_190_p2`, `_cast_reg_380`, …)
/// are kept.
const LN_TAG: &str = "_ln";

/// Fixed marker replacing every erased line number. `~` cannot appear
/// in HLS-generated identifiers, so marked names never collide with
/// real tokens.
const MARKER: &str = "~";

/// Canonicalize verilog text or a file name: replace every
/// `VITIS_LOOP_<line>` with `VITIS_LOOP_~` and every
/// `<prefix>_ln<line>[_<occurrence>]` with `<prefix>_ln~`, keeping all
/// surrounding structure verbatim.
pub fn canonical_verilog(text: &str) -> String {
    let loops = erase_vitis_loop_lines(text);
    erase_ln_units(&loops)
}

/// Canonical form of one verilog file's contents for hashing: line
/// normalization, then the lines sorted, so the statement
/// order — which HLS derives from the line-bearing names — does not
/// count either. Line multiplicity is preserved: inserting or deleting
/// a statement still changes the canonical bytes.
pub fn canonical_verilog_contents(text: &str) -> String {
    let normalized = canonical_verilog(text);
    let mut lines: Vec<&str> = normalized.split('\n').collect();
    lines.sort_unstable();
    lines.join("\n")
}

/// Manifest keys for the artifacts under one directory: artifacts whose
/// normalized paths collide (distinct files, one canonical name) are a
/// sorted MULTISET keyed by canonical content — `path#<ordinal>` with
/// ordinals assigned by sorted hash, so the assignment is deterministic
/// and never silently overwrites. Equal contents keep distinct
/// ordinals, preserving multiplicity.
pub fn keyed_multiset(grouped: BTreeMap<String, Vec<String>>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (path, mut hashes) in grouped {
        hashes.sort_unstable();
        match hashes.as_slice() {
            [hash] => {
                out.insert(path, hash.clone());
            }
            hashes => {
                for (index, hash) in hashes.iter().enumerate() {
                    out.insert(format!("{path}#{}", index + 1), hash.clone());
                }
            }
        }
    }
    out
}

fn erase_vitis_loop_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for_each_line_number(text, LOOP_TAG, |tag_start, num_end| {
        out.push_str(&text[cursor..tag_start]);
        out.push_str(LOOP_TAG);
        out.push_str(MARKER);
        cursor = num_end;
    });
    out.push_str(&text[cursor..]);
    out
}

fn erase_ln_units(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for (unit_start, unit_end) in scan_ln_units(text) {
        out.push_str(&text[cursor..unit_start]);
        out.push_str(LN_TAG);
        out.push_str(MARKER);
        cursor = unit_end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Scan for `tag` immediately followed by an ASCII-digit run, invoking
/// `hit(tag_start, digits_end)` for every occurrence.
fn for_each_line_number<F: FnMut(usize, usize)>(text: &str, tag: &str, mut hit: F) {
    let mut from = 0;
    while let Some(found) = text[from..].find(tag) {
        let tag_start = from + found;
        let num_start = tag_start + tag.len();
        if let Some(len) = leading_digits(&text[num_start..]) {
            hit(tag_start, num_start + len);
            from = num_start + len;
        } else {
            from = num_start;
        }
    }
}

/// The length of the leading ASCII-digit run of `text`, if non-empty.
fn leading_digits(text: &str) -> Option<usize> {
    let len = text.bytes().take_while(u8::is_ascii_digit).count();
    if len == 0 {
        None
    } else {
        Some(len)
    }
}

/// Every `_ln` unit in `text`, leftmost per identifier run of
/// `[A-Za-z0-9_]` characters: the first `_ln<digits>[_<digits>]` inside
/// the run whose prefix is non-empty, as its `(start, end)` byte range.
/// The prefix (everything from the run's start up to `_ln`) is left
/// verbatim in the output.
fn scan_ln_units(text: &str) -> Vec<(usize, usize)> {
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

/// The `(start, end)` of the first `_ln<digits>[_<digits>]` unit in
/// `text[start..end]` with a non-empty prefix, if any.
fn first_ln_unit(text: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    let mut from = start;
    while let Some(found) = text[from..end].find(LN_TAG) {
        let unit_start = from + found;
        if unit_start == start {
            return None; // empty prefix: not an `<op>_ln` name
        }
        let num_start = unit_start + LN_TAG.len();
        let Some(line_len) = leading_digits(&text[num_start..end]) else {
            from = num_start;
            continue;
        };
        let after_line = num_start + line_len;
        // Optional same-line occurrence suffix `_<digits>`, erased with
        // the line it qualifies.
        let unit_end = if text[after_line..end].starts_with('_') {
            leading_digits(&text[after_line + 1..end])
                .map_or(after_line, |len| after_line + 1 + len)
        } else {
            after_line
        };
        return Some((unit_start, unit_end));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Loop lines are erased to one marker; loop ids survive verbatim,
    // so loop count and identity-by-id still distinguish designs.
    #[test]
    fn vitis_loop_lines_are_erased_and_ids_survive() {
        assert_eq!(
            canonical_verilog("Add_Add_Pipeline_VITIS_LOOP_39_1.v"),
            "Add_Add_Pipeline_VITIS_LOOP_~_1.v"
        );
        // Same loop id at different source lines: one normalized name.
        // Keeping the two FILES distinct is the multiset's job.
        assert_eq!(
            canonical_verilog("VITIS_LOOP_100_1 VITIS_LOOP_99_1"),
            "VITIS_LOOP_~_1 VITIS_LOOP_~_1"
        );
        assert_eq!(canonical_verilog("VITIS_LOOP_7"), "VITIS_LOOP_~");
    }

    // Operation names lose line and same-line occurrence but keep
    // structural suffixes (`_fu_190_p2`) and embedded shape
    // (`phi_ln..._phi`) verbatim.
    #[test]
    fn ln_units_erase_line_and_occurrence_keep_structure() {
        assert_eq!(
            canonical_verilog("or_ln49_1_fu_190_p2 or_ln49_fu_172_p2 icmp_ln30"),
            "or_ln~_fu_190_p2 or_ln~_fu_172_p2 icmp_ln~"
        );
        assert_eq!(
            canonical_verilog("icmp_ln99 icmp_ln100 icmp_ln98"),
            "icmp_ln~ icmp_ln~ icmp_ln~"
        );
        assert_eq!(
            canonical_verilog("ap_phi_mux_phi_ln50_phi_fu_100_p4"),
            "ap_phi_mux_phi_ln~_phi_fu_100_p4"
        );
    }

    // Text without the families is untouched, including `_ln` not
    // followed by digits.
    #[test]
    fn unrelated_text_is_untouched() {
        let text = "wire [31:0] a; // _lnfoo VITIS_LOOP\nassign a = data_q_s_din;";
        assert_eq!(canonical_verilog(text), text);
    }

    // The motivating corpus case: the tree's `#line` re-snaps map a
    // construct back to user line 81 while the flattened blob places it
    // at line 96, behind line-94 constructs that are identical in both.
    // Ranking by numeric line assigned the families different ranks;
    // erasing the lines does not.
    #[test]
    fn line_order_inversion_canonicalizes_equal() {
        let flat = concat!(
            "assign trunc_ln96_4_reg_403 = add_ln96_fu_257_p2[63:6];\n",
            "assign trunc_ln94_reg_408 = trunc_ln94_fu_277_p1;\n",
        );
        let tree = concat!(
            "assign trunc_ln94_reg_408 = trunc_ln94_fu_277_p1;\n",
            "assign trunc_ln81_4_reg_403 = add_ln81_fu_257_p2[63:6];\n",
        );
        assert_eq!(
            canonical_verilog_contents(flat),
            canonical_verilog_contents(tree)
        );
    }

    // Sorted, not deduplicated: repeated statements still count.
    #[test]
    fn line_multiplicity_counts() {
        let once = canonical_verilog_contents("assign x = icmp_ln55;\n");
        let twice = canonical_verilog_contents("assign x = icmp_ln55;\nassign x = icmp_ln55;\n");
        assert_ne!(once, twice);
    }

    // Constants and bodies still trip the gate after normalization.
    #[test]
    fn constant_and_body_mutation_detected() {
        let base = canonical_verilog_contents("assign y = icmp_ln55 | 1'b1;\n");
        let flipped = canonical_verilog_contents("assign y = icmp_ln55 | 1'b0;\n");
        let rewritten = canonical_verilog_contents("assign y = icmp_ln55 & 1'b1;\n");
        assert_ne!(base, flipped);
        assert_ne!(base, rewritten);
    }

    // Loop sets: a removed loop renumbers survivors' ids, and the
    // normalized name multiset shrinks either way.
    #[test]
    fn loop_set_changes_detected() {
        let two = canonical_verilog("A_Pipeline_VITIS_LOOP_54_1.v A_Pipeline_VITIS_LOOP_60_2.v");
        let one = canonical_verilog("A_Pipeline_VITIS_LOOP_54_1.v");
        assert_eq!(
            two,
            "A_Pipeline_VITIS_LOOP_~_1.v A_Pipeline_VITIS_LOOP_~_2.v"
        );
        assert_ne!(two, one);
    }

    // The function is a fixed point: markers are never digits, so
    // canonical output re-canonicalizes to itself.
    #[test]
    fn canonicalization_is_deterministic() {
        let text = "assign s = {icmp_ln55, VITIS_LOOP_39_1, or_ln49_1_fu_9};";
        assert_eq!(canonical_verilog(text), canonical_verilog(text));
        assert_eq!(
            canonical_verilog(&canonical_verilog(text)),
            canonical_verilog(text)
        );
    }

    fn grouped(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(path, hashes)| {
                (
                    (*path).to_string(),
                    hashes.iter().map(|hash| (*hash).to_string()).collect(),
                )
            })
            .collect()
    }

    // One artifact on a path keeps the plain key.
    #[test]
    fn singleton_paths_keep_their_key() {
        let keyed = keyed_multiset(grouped(&[("Add/Add.v", &["abc"])]));
        assert_eq!(keyed.len(), 1);
        assert_eq!(keyed["Add/Add.v"], "abc");
    }

    // Colliding artifacts become `#<ordinal>` keys by sorted hash:
    // deterministic, multiplicity-preserving, never overwritten.
    #[test]
    fn colliding_paths_form_a_sorted_multiset() {
        let keyed = keyed_multiset(grouped(&[(
            "Add/Add_Pipeline_VITIS_LOOP_~_1.v",
            &["fff", "000"],
        )]));
        assert_eq!(
            keyed,
            BTreeMap::from([
                (
                    "Add/Add_Pipeline_VITIS_LOOP_~_1.v#1".to_string(),
                    "000".to_string()
                ),
                (
                    "Add/Add_Pipeline_VITIS_LOOP_~_1.v#2".to_string(),
                    "fff".to_string()
                ),
            ])
        );
    }

    // Byte-identical duplicates stay distinct entries: deleting one of
    // two same-content files must register as drift.
    #[test]
    fn duplicate_content_keeps_multiplicity() {
        let keyed = keyed_multiset(grouped(&[("Top.v", &["abc", "abc"])]));
        assert_eq!(
            keyed,
            BTreeMap::from([
                ("Top.v#1".to_string(), "abc".to_string()),
                ("Top.v#2".to_string(), "abc".to_string()),
            ])
        );
    }
}
