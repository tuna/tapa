//! Generated SLASH pre-synthesis and placement hooks.

pub(super) const HOOK_NAME: &str = "tapa_pre_synth.tcl";
pub(super) const PLACE_HOOK_NAME: &str = "tapa_pre_place.tcl";
pub(super) const FLOORPLAN_XDC_NAME: &str = "floorplan.xdc";

pub(super) fn pre_place_hook(has_floorplan: bool, has_hbm: bool) -> String {
    let mut hook =
        include_str!("pre_place.tcl").replace("@TAPA_HAS_HBM@", if has_hbm { "1" } else { "0" });
    if has_floorplan {
        hook.push_str("\nsource [file join $tapa_place_hook_dir floorplan.xdc]\n");
    }
    hook.push_str(include_str!("control_replication.tcl"));
    hook
}

pub(super) fn pre_synth_hook(hbm_indices: &[u32]) -> String {
    let hbm_slices = (!hbm_indices.is_empty()).then(|| {
        include_str!("hbm_slices.tcl").replace(
            "@TAPA_HBM_INDICES@",
            &hbm_indices
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(" "),
        )
    });
    let hbm_slices = hbm_slices.unwrap_or_default();
    let mut hook = "# TAPA: one generated hook for implementation constraints.\n\
     set tapa_hook_dir [file dirname [file normalize [info script]]]\n\
     # Match slashkit's implementation strategy before installing hooks. Its\n\
     # later assignment of the same strategy then preserves these properties.\n\
     set_property strategy Congestion_SSI_SpreadLogic_high [get_runs impl_1]\n\
     # Register the pre-place script so Vivado executes it in the impl_1 run.\n\
     set tapa_place_hook [file join $tapa_hook_dir tapa_pre_place.tcl]\n\
     add_files -fileset utils_1 -norecurse $tapa_place_hook\n\
     set_property STEPS.PLACE_DESIGN.TCL.PRE [list [get_files [file tail $tapa_place_hook]]] [get_runs impl_1]\n"
        .to_string();
    hook.push_str(&hbm_slices);
    hook
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hooks_register_pre_place_without_redefining_the_slash_clock() {
        let hook = pre_synth_hook(&[0, 1, 2]);
        assert!(hook.contains("strategy Congestion_SSI_SpreadLogic_high"));
        assert!(hook.contains("add_files -fileset utils_1"));
        assert!(hook.contains("STEPS.PLACE_DESIGN.TCL.PRE"));
        assert!(hook.contains("[get_files [file tail $tapa_place_hook]]"));
        assert!(!hook.contains("STEPS.OPT_DESIGN.TCL.PRE"));
        assert!(!hook.contains("add_files -fileset constrs_1"));
        assert!(!hook.contains("create_clock"));
        assert!(!hook.contains("assign_bd_address"));
        assert!(!hook.contains("020200010000"));

        assert!(hook.contains("set selected [list 0 1 2]"));
        assert!(hook.contains("axi_register_slice:2.1"));
        assert!(!pre_synth_hook(&[]).contains("set selected [list"));
        assert!(!pre_synth_hook(&[]).contains("tapa_hbm_slice"));

        let placement = pre_place_hook(true, true);
        assert!(placement.contains("USER_SLR_ASSIGNMENT SLR2"));
        assert!(placement.contains("tapa_rm_uram_used > $tapa_rm_uram_capacity"));
        assert!(placement.contains("control-coverage.tsv"));
        assert!(placement.contains("FORCE_MAX_FANOUT 64"));
        assert!(placement.ends_with(include_str!("control_replication.tcl")));
        assert!(!pre_place_hook(false, false).contains("floorplan.xdc"));
    }
}
