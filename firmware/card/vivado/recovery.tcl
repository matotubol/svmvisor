if {$argc != 2} { error "usage: recovery.tcl source_root output_dir" }
set source_root [file normalize [lindex $argv 0]]
set output_dir [file normalize [lindex $argv 1]]
file mkdir $output_dir
create_project -in_memory -part xc7a35tfgg484-2
read_verilog -sv [file join $source_root rtl svmvisor_recovery.sv]
read_xdc [file join $source_root rtl svmvisor_recovery.xdc]
synth_design -top svmvisor_recovery -part xc7a35tfgg484-2
opt_design
place_design
route_design

# An exhaustive primitive allowlist is stronger than searching instance names
# for PCIe/DMA: only constant sources and output buffers may survive routing.
set cells [get_cells -hierarchical -filter {IS_PRIMITIVE == 1}]
set obufs 0
foreach cell $cells {
    set ref [get_property REF_NAME $cell]
    if {$ref ni {GND VCC OBUF}} { error "Recovery policy: forbidden primitive $cell ($ref)" }
    if {$ref eq "OBUF"} { incr obufs }
}
set expected [dict create user_ld1 1 user_ld2 0 ft2232_rst_n 1 ft601_rst_n 0 ft601_wr_n 1 ft601_rd_n 1 ft601_oe_n 1 ft601_siwu_n 1 pcie_wake_n 1]
if {$obufs != 9 || [llength [get_ports]] != 9} { error "Recovery policy: unexpected IO count" }
foreach port [get_ports] {
    if {![dict exists $expected $port] || [get_property DIRECTION $port] ne "OUT"} {
        error "Recovery policy: unexpected port $port"
    }
    set driver [get_cells -of_objects [get_pins -of_objects [get_nets -of_objects $port] -filter {DIRECTION == OUT}]]
    if {[llength $driver] != 1 || [get_property REF_NAME $driver] ne "OBUF"} { error "Recovery policy: $port lacks OBUF" }
    set source [get_cells -of_objects [get_pins -of_objects [get_nets -of_objects [get_pins $driver/I]] -filter {DIRECTION == OUT}]]
    set required [expr {[dict get $expected $port] ? "VCC" : "GND"}]
    if {[llength $source] != 1 || [get_property REF_NAME $source] ne $required} { error "Recovery policy: wrong constant on $port" }
}
report_drc -file [file join $output_dir recovery-drc.rpt]
if {[llength [get_drc_violations -filter {SEVERITY == Error}]] != 0} { error "Recovery DRC errors" }
report_utilization -file [file join $output_dir recovery-utilization.rpt]
write_checkpoint -force [file join $output_dir recovery-routed.dcp]
write_verilog -force [file join $output_dir recovery-routed.v]
write_bitstream -force -bin_file [file join $output_dir svmvisor-recovery.bit]
set report [open [file join $output_dir recovery-policy.txt] w]
puts $report "non_enumerating_netlist_policy=PASS"
puts $report "part=[get_property PART [current_design]]"
puts $report "primitive_count=[llength $cells]"
puts $report "output_buffers=$obufs"
puts $report "constant_outputs=PASS"
puts $report "physical_fixture_test=NOT_RUN"
close $report
close_project
