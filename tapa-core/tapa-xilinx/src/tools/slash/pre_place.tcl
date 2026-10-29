# Keep the shell-facing HBM bridges next to their fixed NoC terminals.
set tapa_place_hook_dir [file dirname [file normalize [info script]]]
set tapa_hbm_bridges [get_cells -quiet -hierarchical -regexp -filter {IS_PRIMITIVE == 0 && NAME =~ "^(.*/)?slash/hbm_sc_[0-9]+$"}]
if {![llength $tapa_hbm_bridges]} {
  error "TAPA V80 link could not resolve the service-shell HBM bridges"
}
set_property USER_SLR_ASSIGNMENT SLR2 $tapa_hbm_bridges

# Check the optimized netlist against the actual RM inventory before placement.
set tapa_rm_urams [get_cells -quiet -hierarchical -regexp -filter {IS_PRIMITIVE == 1 && REF_NAME =~ "^URAM288.*$" && NAME =~ "^(.*/)?slash/.*$"}]
set tapa_rm_uram_sites [get_sites -of_objects [get_pblocks pblock_slash] -filter {SITE_TYPE =~ URAM288*}]
set tapa_rm_uram_used [llength $tapa_rm_urams]
set tapa_rm_uram_capacity [llength $tapa_rm_uram_sites]
if {$tapa_rm_uram_used > $tapa_rm_uram_capacity} {
  error "TAPA V80 RM needs $tapa_rm_uram_used URAM primitives, but the shell exposes $tapa_rm_uram_capacity sites"
}
puts "TAPA V80 RM URAM preflight: $tapa_rm_uram_used/$tapa_rm_uram_capacity; [llength $tapa_hbm_bridges] HBM bridges assigned to SLR2"

# Place generated HBM slices and their reset synchronizer with the bridges.
set tapa_hbm_slices [get_cells -quiet -hierarchical -regexp -filter {IS_PRIMITIVE == 0 && NAME =~ "^(.*/)?slash/tapa_hbm_slice_[0-9]+$"}]
if {[llength $tapa_hbm_slices]} {
  set_property USER_SLR_ASSIGNMENT SLR2 $tapa_hbm_slices
}
set tapa_hbm_reset [get_cells -quiet -hierarchical -regexp -filter {IS_PRIMITIVE == 0 && NAME =~ "^(.*/)?slash/tapa_hbm_reset$"}]
if {[llength $tapa_hbm_reset] == 1} {
  set_property USER_SLR_ASSIGNMENT SLR2 $tapa_hbm_reset
}
