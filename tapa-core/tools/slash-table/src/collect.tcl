# Run in a fresh output directory. All inputs are read-only.
proc collect {checkpoint pblock_name} {
  open_checkpoint $checkpoint
  set part [get_property PART [current_design]]
  if {$part ne "xcv80-lsva4737-2MHP-e-S"} { error "Unsupported part: $part" }
  set pb [get_pblocks $pblock_name]
  if {[llength $pb] != 1 || ![get_property EXCLUDE_PLACEMENT $pb]} {
    error "Expected one exclusive RM pblock"
  }
  set rm [get_cells -of_objects $pb]
  if {[llength $rm] != 1 || ![get_property IS_BLACKBOX $rm]} {
    error "Expected one black-boxed RM in $pb"
  }
  set out [open census.tsv w]
  puts $out "design\t$part\t[version -short]\t$rm"
  foreach range [get_property GRID_RANGES $pb] { puts $out "range\t$range" }
  set cr_slr [dict create]
  foreach slr [lsort [get_slrs]] {
    set index [get_property SLR_INDEX $slr]
    set regions [get_clock_regions -of_objects $slr]
    foreach cr $regions { dict set cr_slr $cr $index }
    # Include NoC-only regions: HBM anchors are outside the CLB fabric.
    foreach cr $regions { puts $out "region\t$index\t$cr" }
    puts $out "slr\t$index\t[get_property NUM_TOP_SLLS $slr]\t[get_property NUM_BOT_SLLS $slr]"
  }
  set sites [get_sites -of_objects $pb]
  set counts [dict create]
  foreach kind [get_property SITE_TYPE $sites] cr [get_property CLOCK_REGION $sites] \
      prohibited [get_property PROHIBIT $sites] used [get_property IS_USED $sites] {
    if {$kind ni {SLICEL SLICEM DSP58_PRIMARY RAMB18_L RAMB18_U URAM288}} { continue }
    # Whole occupied sites are excluded conservatively, including partially used slices.
    set state [expr {$prohibited || $used ? "excluded" : "available"}]
    dict incr counts [list [dict get $cr_slr $cr] $kind $state]
  }
  dict for {key count} $counts { puts $out "resource\t[join $key \t]\t$count" }
  set noc_sites [get_sites -filter {SITE_TYPE == NOC_NMU512 || SITE_TYPE == NOC_NSU512 || SITE_TYPE == NOC_NMU_HBM2E}]
  foreach site $noc_sites cr [get_property CLOCK_REGION $noc_sites] {
    puts $out "site\t$site\t[dict get $cr_slr $cr]\t$cr"
  }
  set pins [get_pins -of_objects $rm]
  set pin_file [open pins.tsv w]
  foreach pin [get_property REF_PIN_NAME $pins] direction [get_property DIRECTION $pins] {
    puts $pin_file "$pin\t$direction"
  }
  close $pin_file
  set requests [open requests.tsv r]
  while {[gets $requests line] >= 0} {
    lassign [split $line \t] kind name signal
    set pin [get_pins -quiet $rm/$signal]
    if {[llength $pin] != 1} { error "Missing or ambiguous RM pin $signal" }
    switch -- $kind {
      hbm {
        set nets [get_nets -segments -of_objects $pin]
        set cells [get_cells -of_objects [get_pins -leaf -of_objects $nets -filter {DIRECTION == IN}]]
        if {[llength $cells] != 1 || [get_property REF_NAME $cells] ne "NOC_NMU_HBM2E"} {
          error "Cannot trace $name directly to one HBM NMU: $cells"
        }
        puts $out "hbm\t$name\t[get_property LOC $cells]"
      }
      clock {
        set clocks [get_clocks -of_objects $pin]
        if {[llength $clocks] != 1} { error "Missing or ambiguous clock at $name" }
        puts $out "clock\t$name\t[get_property PERIOD $clocks]"
      }
      default { error "Unknown request $kind" }
    }
  }
  close $requests
  close $out
  write_noc_solution -force noc.ncr
  write_noc_qos -file noc.nts
  report_utilization -file utilization.rpt
  report_utilization -pblocks $pb -file pblock-utilization.rpt
  close_design
}

if {[catch {collect {*}$argv} message options]} {
  puts stderr [dict get $options -errorinfo]
  exit 1
}
exit 0
