# TAPA: time the reconfigurable module at the requested kernel period instead
# of the static shell default (clkout1_primitive_2 = 5.000 ns).
set tapa_clk [get_clocks clkout1_primitive_2]
set tapa_src [get_pins [get_property SOURCE_PINS $tapa_clk]]
create_clock -period 3.333 -name tapa_user_clk $tapa_src
