# Insert Full AXI register slices on the connected HBM interfaces. Payload,
# VALID and READY are registered on every channel, preserving AXI semantics.
set selected [list @TAPA_HBM_INDICES@]
set hbm_clock [get_bd_ports static_region_clk]
if {[llength $hbm_clock] != 1} {error "HBM clock port missing"}
if {[get_property CONFIG.FREQ_HZ $hbm_clock] != 400000000} {
  error "TAPA V80 requires the pinned 400 MHz HBM clock"
}
set first [format hbm_sc_%02d [lindex $selected 0]]
set clock_net [get_bd_nets -of_objects [get_bd_pins $first/aclk1]]
set reset_net [get_bd_nets -of_objects [get_bd_pins $first/aresetn]]
if {[llength $clock_net] != 1 || [llength $reset_net] != 1} {
  error "ambiguous HBM clock/reset connection"
}
create_bd_cell -type ip -vlnv xilinx.com:ip:proc_sys_reset:5.0 tapa_hbm_reset
set_property -dict [list CONFIG.C_EXT_RESET_HIGH 0 CONFIG.C_AUX_RESET_HIGH 1] [get_bd_cells tapa_hbm_reset]
create_bd_cell -type ip -vlnv xilinx.com:ip:xlconstant:1.1 tapa_hbm_one
set_property -dict [list CONFIG.CONST_WIDTH 1 CONFIG.CONST_VAL 1] [get_bd_cells tapa_hbm_one]
create_bd_cell -type ip -vlnv xilinx.com:ip:xlconstant:1.1 tapa_hbm_zero
set_property -dict [list CONFIG.CONST_WIDTH 1 CONFIG.CONST_VAL 0] [get_bd_cells tapa_hbm_zero]
connect_bd_net -net $clock_net [get_bd_pins tapa_hbm_reset/slowest_sync_clk]
connect_bd_net -net $reset_net [get_bd_pins tapa_hbm_reset/ext_reset_in]
connect_bd_net [get_bd_pins tapa_hbm_one/dout] [get_bd_pins tapa_hbm_reset/dcm_locked]
connect_bd_net [get_bd_pins tapa_hbm_zero/dout] [get_bd_pins tapa_hbm_reset/aux_reset_in] [get_bd_pins tapa_hbm_reset/mb_debug_sys_rst]
foreach index $selected {
  set bridge [format hbm_sc_%02d $index]
  set name [format tapa_hbm_slice_%02d $index]
  set mi [get_bd_intf_pins $bridge/M00_AXI]
  set net [get_bd_intf_nets -of_objects $mi]
  set external [get_bd_intf_ports -of_objects $net]
  if {[llength $mi] != 1 || [llength $net] != 1 || [llength $external] != 1 || [get_property NAME $external] ne [format HBM_AXI_%02d $index]} {
    error "unexpected HBM$index interface topology"
  }
  if {[get_bd_nets -of_objects [get_bd_pins $bridge/aclk1]] ne $clock_net || [get_bd_nets -of_objects [get_bd_pins $bridge/aresetn]] ne $reset_net} {
    error "HBM$index is in a different clock/reset domain"
  }
  create_bd_cell -type ip -vlnv xilinx.com:ip:axi_register_slice:2.1 $name
  set_property -dict [list CONFIG.PROTOCOL AXI4 CONFIG.DATA_WIDTH 256 CONFIG.ADDR_WIDTH 64 CONFIG.ID_WIDTH 0 CONFIG.REG_AW 1 CONFIG.REG_W 1 CONFIG.REG_B 1 CONFIG.REG_AR 1 CONFIG.REG_R 1] [get_bd_cells $name]
  disconnect_bd_intf_net $net $mi
  connect_bd_intf_net $mi [get_bd_intf_pins $name/S_AXI]
  connect_bd_intf_net $external [get_bd_intf_pins $name/M_AXI]
  connect_bd_net -net $clock_net [get_bd_pins $name/aclk]
  connect_bd_net [get_bd_pins tapa_hbm_reset/peripheral_aresetn] [get_bd_pins $name/aresetn]
}
# Vivado 2025.2 initialization changes AW/B/AR to Light. Reassert Full after
# connectivity validation, then verify every channel explicitly.
validate_bd_design
foreach index $selected {
  set cell [get_bd_cells [format tapa_hbm_slice_%02d $index]]
  set_property -dict [list CONFIG.REG_AW 1 CONFIG.REG_W 1 CONFIG.REG_B 1 CONFIG.REG_AR 1 CONFIG.REG_R 1] $cell
}
save_bd_design
set out [open [file join $tapa_hook_dir hbm-slices.tsv] w]
puts $out "port\tslice\tdata_width\tfrequency_hz\tREG_AW\tREG_W\tREG_B\tREG_AR\tREG_R"
foreach index $selected {
  set cell [get_bd_cells [format tapa_hbm_slice_%02d $index]]
  set row [list $index [get_property NAME $cell] [get_property CONFIG.DATA_WIDTH $cell] [get_property CONFIG.FREQ_HZ [get_bd_pins $cell/aclk]]]
  foreach channel {AW W B AR R} {
    set mode [get_property CONFIG.REG_$channel $cell]
    if {$mode != 1} {error "register slice channel $channel is not in Full mode"}
    lappend row $mode
  }
  if {[get_property CONFIG.DATA_WIDTH $cell] != 256} {error "HBM data width changed"}
  puts $out [join $row \t]
}
close $out
puts "TAPA V80 inserted [llength $selected] Full HBM register slices"
