//! Generated SLASH pre-synthesis hook and implementation-only clock XDC.

pub(super) const HOOK_NAME: &str = "tapa_pre_synth.tcl";
pub(super) const CLOCK_XDC_NAME: &str = "tapa_user_clock.xdc";
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

pub(super) fn pre_synth_hook(has_floorplan: bool) -> String {
    let floorplan = has_floorplan.then_some(
        "# Apply TAPA placement after the RM is linked and before placement.\n\
         set tapa_floorplan_xdc [file join $tapa_hook_dir floorplan.xdc]\n\
         set_property STEPS.PLACE_DESIGN.TCL.PRE [list $tapa_floorplan_xdc] [get_runs impl_1]\n",
    );
    format!(
        "# TAPA: one generated hook for implementation constraints.\n\
         set tapa_hook_dir [file dirname [file normalize [info script]]]\n\
         {}\
         set tapa_user_clock_xdc [file join $tapa_hook_dir tapa_user_clock.xdc]\n\
         add_files -fileset constrs_1 -norecurse $tapa_user_clock_xdc\n\
         set_property USED_IN_SYNTHESIS false [get_files $tapa_user_clock_xdc]\n\
         set_property USED_IN_IMPLEMENTATION true [get_files $tapa_user_clock_xdc]\n",
        floorplan.unwrap_or_default()
    )
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
    fn one_hook_adds_implementation_xdc_and_optional_place_hook() {
        let hook = pre_synth_hook(true);
        assert!(hook.contains("STEPS.PLACE_DESIGN.TCL.PRE"));
        assert!(!hook.contains("STEPS.OPT_DESIGN.TCL.PRE"));
        assert!(hook.contains("add_files -fileset constrs_1"));
        assert!(hook.contains("USED_IN_SYNTHESIS false"));
        assert!(hook.contains("USED_IN_IMPLEMENTATION true"));
        assert!(!hook.contains("assign_bd_address"));
        assert!(!hook.contains("020200010000"));
        assert!(!pre_synth_hook(false).contains("PLACE_DESIGN"));
    }
}
