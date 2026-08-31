//! Canonical form of Vitis-HLS-generated verilog, for parity hashing.
//!
//! Vitis HLS bakes source-debug attribution into the names it
//! generates: line numbers (`icmp_ln55`, `or_ln49_1`), loop-bearing
//! module/file names (`Add_Pipeline_VITIS_LOOP_38_1.v`), SSA-value
//! spellings (`exitcond262`, `pid_4`), and derived signals keyed on
//! those names (`ap_sig_allocacmp_pid_6`). Two syntheses of the *same*
//! design whose C++ differs only in line layout — e.g. the campaign's
//! mirrored tree with its `#line` re-snaps versus the flattened blob —
//! produce RTL that differs in exactly those tokens and nothing else
//! (established empirically by diffing such generations across the
//! parity apps); the only other byte difference is the order of
//! independent statements, which HLS keys on those same names.
//!
//! The two layouts do not even agree on the *order* of the line
//! numbers: a `#line` re-snap maps one construct back to user line 81
//! while the flattened blob places it at line 96, behind a construct
//! that stays at line 94 in both. And the attribution itself can
//! differ: the flattened blob attributes one allocation to the source
//! variable (`pid_4_fu_95_p3`) where the tree attributes it to a
//! source-line operation (`select_ln9_fu_95_p3`), shifting every later
//! SSA occurrence of the variable by one. So canonicalization ranks
//! nothing by numeric line — that was the previous design and it
//! drifted — and keys no identity on the debug spelling. It erases the
//! debug-derived components of every family with a fixed marker, keeps
//! every structural component verbatim (loop ids, `_fu_N`/`_reg_N`
//! allocation numbers, port and module names, `_cast`, …), and sorts
//! the lines of the result, so semantically identical RTL hashes equal
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
//! - A name-only change in front of a stable allocation (`pid_4` →
//!   `select_ln9`, `exitcond262` → `exitcond252`) is invisible by
//!   design: the allocation number is the identity, and the operation
//!   itself is still gated by the RTL body around the name.
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
//! - Allocation-compare occurrences are erased only in their
//!   underscore-separated spelling; a glued shift (`p_load20` →
//!   `p_load21`) still registers as drift — failing closed, never
//!   matching two designs falsely.
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
/// are kept. A line-attributed name with NO number (`trunc_ln`) is the
/// same family with an unknown line (HLS omits the digits when the
/// operation has no debug location — the flattened blob, whose `-P`
/// strip lost some, versus the tree's `#line`-restored one), so a bare
/// `_ln` at the identifier's end erases to the same marker.
const LN_TAG: &str = "_ln";

/// Infixes of the HLS structural allocation suffix: `_fu_<n>` for a
/// function-unit result and `_reg_<n>` for a register, as in
/// `select_ln9_fu_95_p3` or `pid_5_reg_355_pp0_iter1_reg`. The digits
/// are the allocation's identity — identical across syntheses of the
/// same design — while everything before the suffix is debug/source
/// attribution (operator + line, or variable + SSA occurrence) that the
/// two source layouts spell differently.
const ALLOCATION_TAGS: [&str; 2] = ["_fu_", "_reg_"];

/// Prefix of HLS allocation-compare signals: `ap_sig_allocacmp_<name>`
/// — the dependency-check wire HLS derives from a value's debug name,
/// e.g. `ap_sig_allocacmp_pid_6`. The wire carries no allocation
/// suffix; its layout-varying part is the trailing `_<occurrence>`
/// digits, the same source-layout sensitivity the `_ln` family erases.
/// The value name (`pid`, `j_load`) is stable and kept.
const ALLOCACMP_TAG: &str = "ap_sig_allocacmp_";

/// Fixed marker replacing every erased debug-derived component. `~`
/// cannot appear in HLS-generated identifiers, so marked names never
/// collide with real tokens.
const MARKER: &str = "~";

/// Canonicalize verilog text or a file name: erase the debug prefix in
/// front of every `_fu_<n>`/`_reg_<n>` allocation suffix, then every
/// `VITIS_LOOP_<line>` and `<prefix>_ln<line>[_<occurrence>]`, then
/// the trailing occurrence of every allocation-compare signal, keeping
/// all surrounding structure verbatim.
///
/// The debug-prefix erasure must run before the `_ln` erasure: the
/// marker is not an identifier character, so a `_ln`-marked name
/// (`select_ln~_fu_95_p3`) no longer scans as one identifier and its
/// prefix could not be erased anymore.
pub fn canonical_verilog(text: &str) -> String {
    let prefixed = erase_debug_prefixes(text);
    let loops = erase_vitis_loop_lines(&prefixed);
    let lines = erase_ln_units(&loops);
    erase_allocacmp_occurrences(&lines)
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

/// Replace the debug prefix of every identifier that carries an
/// allocation suffix: `pid_4_fu_95_p3` and `select_ln9_fu_95_p3` both
/// become `~_fu_95_p3`. Identifiers without a suffix — module names,
/// port names, plain signals — are left verbatim.
fn erase_debug_prefixes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for_each_identifier_run(text, |start, end| {
        if let Some(suffix_start) = allocation_suffix_start(text, start, end) {
            out.push_str(&text[cursor..start]);
            out.push_str(MARKER);
            cursor = suffix_start;
        }
    });
    out.push_str(&text[cursor..]);
    out
}

/// The byte offset of the first `_fu_<digits>`/`_reg_<digits>` unit in
/// the identifier `text[start..end]` that has a non-empty prefix
/// before it. HLS emits at most one allocation unit per name (on every
/// corpus generation examined), so the first is also the only; a unit
/// at the identifier's start has no debug prefix to erase.
fn allocation_suffix_start(text: &str, start: usize, end: usize) -> Option<usize> {
    let mut leftmost = None;
    for tag in ALLOCATION_TAGS {
        let mut from = start;
        while let Some(found) = text[from..end].find(tag) {
            let unit_start = from + found;
            let num_start = unit_start + tag.len();
            if unit_start > start && leading_digits(&text[num_start..end]).is_some() {
                leftmost =
                    Some(leftmost.map_or(unit_start, |current: usize| current.min(unit_start)));
                break;
            }
            from = unit_start + 1;
        }
    }
    leftmost
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

/// Erase the trailing `_<occurrence>` of every allocation-compare
/// signal: `ap_sig_allocacmp_pid_6` and `ap_sig_allocacmp_pid_5` both
/// become `ap_sig_allocacmp_pid~`. Glued digits (`p_load20`) and
/// non-numeric tails (`j_load`) are other spellings of the family that
/// today's corpus never varies on, so they stay verbatim.
fn erase_allocacmp_occurrences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for_each_identifier_run(text, |start, end| {
        let name = &text[start..end];
        if !name.starts_with(ALLOCACMP_TAG) {
            return;
        }
        let Some(underscore) = name.rfind('_') else {
            return;
        };
        let occurrence_start = start + underscore;
        if leading_digits(&text[occurrence_start + 1..end]).is_some() {
            out.push_str(&text[cursor..occurrence_start]);
            out.push_str(MARKER);
            cursor = end;
        }
    });
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

/// Invokes `hit(start, end)` for every maximal run of identifier
/// characters `[A-Za-z0-9_]` in `text`, left to right.
fn for_each_identifier_run<F: FnMut(usize, usize)>(text: &str, mut hit: F) {
    let bytes = text.as_bytes();
    let is_id = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
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
            hit(start, index);
        }
        index += 1;
    }
}

/// Every `_ln` unit in `text`, leftmost per identifier run: the first
/// `_ln<digits>[_<digits>]` inside the run whose prefix is non-empty,
/// as its `(start, end)` byte range. The prefix (everything from the
/// run's start up to `_ln`) is left verbatim in the output.
fn scan_ln_units(text: &str) -> Vec<(usize, usize)> {
    let mut units = Vec::new();
    for_each_identifier_run(text, |start, end| {
        if let Some(unit) = first_ln_unit(text, start, end) {
            units.push(unit);
        }
    });
    units
}

/// The `(start, end)` of the first `_ln<digits>[_<digits>]` unit in
/// `text[start..end]` with a non-empty prefix, if any. A `_ln` ending
/// the identifier is the unknown-line spelling of the same family.
fn first_ln_unit(text: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    let mut from = start;
    while let Some(found) = text[from..end].find(LN_TAG) {
        let unit_start = from + found;
        if unit_start == start {
            return None; // empty prefix: not an `<op>_ln` name
        }
        let num_start = unit_start + LN_TAG.len();
        let Some(line_len) = leading_digits(&text[num_start..end]) else {
            // Unknown line: only the bare `_ln` ending the identifier
            // is the family. Names whose `_ln` is followed by more
            // structure reach here only without digits — allocation
            // suffixes were already erased with their debug prefix —
            // and `_ln` inside another identifier (`or_lnfoo`) never
            // was; an already-marked unit is left alone, keeping
            // canonicalization a fixed point.
            if num_start == end && !text[end..].starts_with(MARKER) {
                return Some((unit_start, num_start));
            }
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

    // The motivating corpus case: the flattened blob attributes an
    // allocation to the source variable (`pid_4`), the tree to a
    // source-line operation (`select_ln9`); SSA-value spellings
    // (`exitcond262`) and register-allocated names shift the same way.
    // The allocation suffix is the identity; the debug prefix is not.
    #[test]
    fn debug_prefixes_before_allocations_canonicalize_equal() {
        assert_eq!(canonical_verilog("pid_4_fu_95_p3"), "~_fu_95_p3");
        assert_eq!(canonical_verilog("select_ln9_fu_95_p3"), "~_fu_95_p3");
        assert_eq!(
            canonical_verilog("exitcond262_fu_52_p2"),
            canonical_verilog("exitcond252_fu_52_p2")
        );
        // The whole suffix is preserved, including a trailing `_reg`
        // that is part of the pipeline register's name.
        assert_eq!(
            canonical_verilog("pid_5_reg_355_pp0_iter1_reg"),
            canonical_verilog("pid_4_reg_355_pp0_iter1_reg")
        );
        assert_eq!(
            canonical_verilog("pid_5_reg_355_pp0_iter1_reg"),
            "~_reg_355_pp0_iter1_reg"
        );
    }

    // Allocation numbers still gate: adding, moving, or renumbering an
    // allocation changes the canonical form even with identical debug
    // prefixes.
    #[test]
    fn allocation_number_changes_detected() {
        let base = canonical_verilog_contents(concat!(
            "wire [10:0] pid_4_fu_95_p3;\n",
            "wire [10:0] pid_1_reg_120;\n",
            "assign pid_fu_34 <= pid_4_fu_95_p3;\n",
        ));
        let renumbered = canonical_verilog_contents(concat!(
            "wire [10:0] pid_4_fu_96_p3;\n",
            "wire [10:0] pid_1_reg_120;\n",
            "assign pid_fu_34 <= pid_4_fu_96_p3;\n",
        ));
        let moved = canonical_verilog_contents(concat!(
            "wire [10:0] pid_4_fu_95_p3;\n",
            "wire [10:0] pid_1_reg_121;\n",
            "assign pid_fu_34 <= pid_4_fu_95_p3;\n",
        ));
        assert_ne!(base, renumbered);
        assert_ne!(base, moved);
    }

    // Operation names lose line and same-line occurrence; identifiers
    // WITHOUT an allocation suffix keep their `_ln` erasure and every
    // embedded shape verbatim.
    #[test]
    fn ln_units_erase_line_and_occurrence_keep_structure() {
        assert_eq!(
            canonical_verilog("or_ln49_1_fu_190_p2 or_ln49_fu_172_p2 icmp_ln30"),
            "~_fu_190_p2 ~_fu_172_p2 icmp_ln~"
        );
        assert_eq!(
            canonical_verilog("icmp_ln99 icmp_ln100 icmp_ln98"),
            "icmp_ln~ icmp_ln~ icmp_ln~"
        );
        assert_eq!(
            canonical_verilog("ap_phi_mux_phi_ln50_phi_fu_100_p4"),
            "~_fu_100_p4"
        );
    }

    // An unknown line and a known one are the same family: the flattened
    // blob emits `trunc_ln` where the tree's `#line`-restored source
    // emits `trunc_ln9`, with identical allocation numbers around them.
    #[test]
    fn unknown_line_and_known_line_names_canonicalize_equal() {
        let flat = "wire signed [61:0] trunc_ln_fu_113_p4;\nreg [61:0] trunc_ln_reg_144;";
        let tree = "wire signed [61:0] trunc_ln9_fu_113_p4;\nreg [61:0] trunc_ln9_reg_144;";
        assert_eq!(canonical_verilog(flat), canonical_verilog(tree));
        assert_eq!(
            canonical_verilog(flat),
            "wire signed [61:0] ~_fu_113_p4;\nreg [61:0] ~_reg_144;"
        );
    }

    // Text without the families is untouched, including `_ln`/`_fu`
    // spellings that are not the families' forms: `_ln` followed by
    // letters, `_fu` without digits, digits glued before an underscore.
    #[test]
    fn unrelated_text_is_untouched() {
        let text = concat!(
            "wire [31:0] a; // _lnfoo _fu_ _reg_ 95_fu\n",
            "assign a = data_q_s_din;\n"
        );
        assert_eq!(canonical_verilog(text), text);
    }

    // Module and port identities carry no allocation suffix, so they
    // are never erased: renaming either still registers as drift.
    #[test]
    fn module_and_port_identities_survive() {
        let module = "module Control_done_RAM_AUTO_1R1W(input clk, input we0);\nendmodule\n";
        assert_eq!(canonical_verilog(module), module);
        let remodeled = "module Control_done_RAM_AUTO_1R1W(input clk, input re0);\nendmodule\n";
        let renamed = "module Control_done_RAM_AUTO_1R1Wx(input clk, input we0);\nendmodule\n";
        let base = canonical_verilog_contents(module);
        assert_ne!(base, canonical_verilog_contents(remodeled));
        assert_ne!(base, canonical_verilog_contents(renamed));
    }

    // Allocation-compare signals: the trailing occurrence erases, the
    // compared value's name stays, other spellings stay verbatim.
    #[test]
    fn allocacmp_occurrence_erases_but_value_name_stays() {
        assert_eq!(
            canonical_verilog("ap_sig_allocacmp_pid_6"),
            "ap_sig_allocacmp_pid~"
        );
        assert_eq!(
            canonical_verilog("ap_sig_allocacmp_pid_6"),
            canonical_verilog("ap_sig_allocacmp_pid_5")
        );
        assert_ne!(
            canonical_verilog("ap_sig_allocacmp_pid_6"),
            canonical_verilog("ap_sig_allocacmp_eid_1")
        );
        assert_eq!(
            canonical_verilog("ap_sig_allocacmp_j_load_1"),
            "ap_sig_allocacmp_j_load~"
        );
        // Glued occurrence digits and bare names are not erased.
        assert_eq!(
            canonical_verilog("ap_sig_allocacmp_p_load20 ap_sig_allocacmp_pid"),
            "ap_sig_allocacmp_p_load20 ap_sig_allocacmp_pid"
        );
    }

    // The tree's `#line` re-snaps map a construct back to user line 81
    // while the flattened blob places it at line 96, behind line-94
    // constructs that are identical in both. Ranking by numeric line
    // assigned the families different ranks; erasing the lines does not.
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

    // The gather_resp corpus shape: an allocation attributed to the
    // variable in one layout and to a line-operation in the other,
    // shifting the variable's later SSA occurrences (including its
    // allocation-compare signal) by one.
    #[test]
    fn attribution_shift_canonicalizes_equal() {
        let flat = concat!(
            "reg [10:0] ap_sig_allocacmp_pid_6;\n",
            "wire [10:0] pid_7_fu_136_p2;\n",
            "assign pid_7_fu_136_p2 = (ap_sig_allocacmp_pid_6 + 11'd1);\n",
        );
        let tree = concat!(
            "reg [10:0] ap_sig_allocacmp_pid_5;\n",
            "wire [10:0] pid_6_fu_136_p2;\n",
            "assign pid_6_fu_136_p2 = (ap_sig_allocacmp_pid_5 + 11'd1);\n",
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
        let text = "assign s = {icmp_ln55, VITIS_LOOP_39_1, or_ln49_1_fu_9, ap_sig_allocacmp_i_2};";
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
