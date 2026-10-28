//! Generated SLASH pre-synthesis hook and implementation-only clock XDC.

pub(super) const HOOK_NAME: &str = "tapa_pre_synth.tcl";
pub(super) const CLOCK_XDC_NAME: &str = "tapa_user_clock.xdc";
pub(super) const PLACE_HOOK_NAME: &str = "tapa_pre_place.tcl";
pub(super) const FLOORPLAN_XDC_NAME: &str = "floorplan.xdc";

pub(super) fn user_clock_xdc(target_mhz: u32) -> String {
    let period_ns = 1000.0 / f64::from(target_mhz);
    format!(
        "# TAPA: time the reconfigurable module at the requested kernel period instead\n\
         # of the static shell default (clkout1_primitive_2 = 5.000 ns).\n\
         set tapa_clk [get_clocks clkout1_primitive_2]\n\
         set tapa_src [get_pins [get_property SOURCE_PINS $tapa_clk]]\n\
         create_clock -period {period_ns:.3} -name tapa_user_clk $tapa_src\n"
    )
}

pub(super) fn pre_place_hook(has_floorplan: bool) -> String {
    let mut hook = include_str!("pre_place.tcl").to_owned();
    if has_floorplan {
        hook.push_str("\nsource [file join $tapa_place_hook_dir floorplan.xdc]\n");
    }
    hook
}

pub(super) fn pre_synth_hook() -> String {
    "# TAPA: one generated hook for implementation constraints.\n\
     set tapa_hook_dir [file dirname [file normalize [info script]]]\n\
     # Match slashkit's implementation strategy before installing hooks. Its\n\
     # later assignment of the same strategy then preserves these properties.\n\
     set_property strategy Congestion_SSI_SpreadLogic_high [get_runs impl_1]\n\
     # Register the pre-place script so Vivado executes it in the impl_1 run.\n\
     set tapa_place_hook [file join $tapa_hook_dir tapa_pre_place.tcl]\n\
     add_files -fileset utils_1 -norecurse $tapa_place_hook\n\
     set_property STEPS.PLACE_DESIGN.TCL.PRE [list [get_files [file tail $tapa_place_hook]]] [get_runs impl_1]\n\
     set tapa_user_clock_xdc [file join $tapa_hook_dir tapa_user_clock.xdc]\n\
     add_files -fileset constrs_1 -norecurse $tapa_user_clock_xdc\n\
     set_property USED_IN_SYNTHESIS false [get_files $tapa_user_clock_xdc]\n\
     set_property USED_IN_IMPLEMENTATION true [get_files $tapa_user_clock_xdc]\n"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_xdc_matches_the_measured_slash_hook_shape() {
        assert_eq!(
            user_clock_xdc(300),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/testdata/slash/tapa_user_clock.xdc"
            )),
        );
        let xdc = user_clock_xdc(250);
        assert!(xdc.contains("get_property SOURCE_PINS"));
        assert!(xdc.contains("create_clock -period 4.000 -name tapa_user_clk"));
    }

    #[test]
    fn hooks_register_pre_place_and_implementation_clock() {
        let hook = pre_synth_hook();
        assert!(hook.contains("strategy Congestion_SSI_SpreadLogic_high"));
        assert!(hook.contains("add_files -fileset utils_1"));
        assert!(hook.contains("STEPS.PLACE_DESIGN.TCL.PRE"));
        assert!(hook.contains("[get_files [file tail $tapa_place_hook]]"));
        assert!(!hook.contains("STEPS.OPT_DESIGN.TCL.PRE"));
        assert!(hook.contains("add_files -fileset constrs_1"));
        assert!(hook.contains("USED_IN_SYNTHESIS false"));
        assert!(hook.contains("USED_IN_IMPLEMENTATION true"));
        assert!(!hook.contains("assign_bd_address"));
        assert!(!hook.contains("020200010000"));

        let placement = pre_place_hook(true);
        assert!(placement.contains("USER_SLR_ASSIGNMENT SLR2"));
        assert!(placement.contains("tapa_rm_uram_used > $tapa_rm_uram_capacity"));
        assert!(placement.ends_with("source [file join $tapa_place_hook_dir floorplan.xdc]\n"));
        assert!(!pre_place_hook(false).contains("floorplan.xdc"));
    }
}
