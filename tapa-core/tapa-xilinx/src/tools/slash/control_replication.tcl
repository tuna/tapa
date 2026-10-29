# Request clock-region replication for eligible high-fanout fabric controls.
# Existing global clock buffers retain their topology.
set out [open [file join $tapa_place_hook_dir control-coverage.tsv] w]
puts $out "net\ttype\tlogical_fanout\tdriver\tdriver_ref\tcontrol_loads\taction"
set seen [dict create]
set tapa_control_enabled 0
set candidates [get_nets -hierarchical -regexp -filter {FLAT_PIN_COUNT > 128 && NAME =~ "^(.*/)?slash/.*$" && TYPE == SIGNAL}]
foreach candidate $candidates {
  set nets [get_nets -top_net_of_hierarchical_group -segments $candidate]
  if {[llength $nets] != 1} {error "ambiguous physical net for $candidate"}
  set net [lindex $nets 0]
  set name [get_property NAME $net]
  if {[dict exists $seen $name]} {continue}
  dict set seen $name 1
  if {[get_property IS_ROUTE_FIXED $net]} {continue}
  set segments [get_nets -segments $net]
  set drivers [get_pins -quiet -leaf -of_objects $segments -filter {DIRECTION == OUT}]
  if {[llength $drivers] != 1} {continue}
  set driver [lindex $drivers 0]
  set cell [get_cells -of_objects $driver]
  set cell_name [get_property NAME $cell]
  set ref [get_property REF_NAME $cell]
  set controls [get_pins -quiet -leaf -of_objects $segments -filter {DIRECTION == IN && (IS_ENABLE == 1 || IS_RESET == 1 || IS_SET == 1 || IS_CLEAR == 1 || IS_PRESET == 1 || IS_SETRESET == 1)}]
  if {![regexp {^(FD(RE|SE|CE|PE)|LUT[1-6](_2)?)$} $ref]} {continue}
  if {![regexp {(^|/)slash/} $cell_name]} {continue}
  if {[get_property DONT_TOUCH $cell] in {1 true}} {continue}
  if {![llength $controls]} {
    set strobe 0
    foreach segment $segments {
      if {[regexp {/control_s_axi_U/.*WSTRB} [get_property NAME $segment]]} {set strobe 1; break}
    }
    if {!$strobe} {continue}
  }
  set_property MAX_FANOUT_MODE CLOCK_REGION $net
  set_property FORCE_MAX_FANOUT 64 $net
  puts $out "$name\t[get_property TYPE $net]\t[expr {[get_property FLAT_PIN_COUNT $net] - 1}]\t[get_property NAME $driver]\t$ref\t[llength $controls]\trequest_CLOCK_REGION_FORCE_MAX_FANOUT_64"
  incr tapa_control_enabled
}
close $out
puts "TAPA control coverage: $tapa_control_enabled fabric replication requests"
