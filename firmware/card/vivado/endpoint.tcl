if {$argc < 5 || $argc > 7} { error "usage: endpoint.tcl source_root output_dir rom_memory fpga_id rom_id ?payload_enabled? ?rom_bytes?" }
set payload_enabled 0
set rom_bytes 8192
if {$argc >= 6} {set payload_enabled [lindex $argv 5]}
if {$argc >= 7} {set rom_bytes [lindex $argv 6]}
if {$payload_enabled ni {0 1}} {error "payload_enabled must be 0 or 1"}
if {![string is integer -strict $rom_bytes] || $rom_bytes < 8192 || $rom_bytes > 131072 || ($rom_bytes & ($rom_bytes-1)) != 0} {error "ROM size must be a power of two from 8192 to 131072"}
set rom_kib [expr {$rom_bytes / 1024}]
set payload_bar [expr {$payload_enabled ? "true" : "false"}]
set fpga_id [lindex $argv 3]
set rom_id [lindex $argv 4]
foreach id [list $fpga_id $rom_id] { if {![regexp {^[0-9a-f]{16}$} $id]} { error "Invalid build ID" } }
set source_root [file normalize [lindex $argv 0]]
set output_dir [file normalize [lindex $argv 1]]
set rom_memory [file normalize [lindex $argv 2]]
cd $output_dir
create_project -in_memory -part xc7a35tfgg484-2
set_property XPM_LIBRARIES {XPM_CDC} [current_project]
file mkdir [file join $output_dir ip]
create_ip -name pcie_7x -vendor xilinx.com -library ip -version 3.3 -module_name svmvisor_pcie_core -dir [file join $output_dir ip]
set ip [get_ips svmvisor_pcie_core]
set desired [list CONFIG.mode_selection Advanced CONFIG.Device_Port_Type PCI_Express_Endpoint_device \
    CONFIG.Maximum_Link_Width X1 CONFIG.Link_Speed 2.5_GT/s CONFIG.Interface_Width 64_bit \
    CONFIG.User_Clk_Freq 62.5 CONFIG.Max_Payload_Size 128_bytes \
    CONFIG.Bar0_Enabled true CONFIG.Bar0_Type Memory CONFIG.Bar0_64bit false CONFIG.Bar0_Prefetchable false \
    CONFIG.Bar0_Scale Kilobytes CONFIG.Bar0_Size 4 \
    CONFIG.Bar1_Enabled $payload_bar CONFIG.Bar1_Type Memory CONFIG.Bar1_64bit false CONFIG.Bar1_Prefetchable false \
    CONFIG.Bar1_Scale Megabytes CONFIG.Bar1_Size 1 CONFIG.Bar2_Enabled false CONFIG.Bar3_Enabled false CONFIG.Bar4_Enabled false CONFIG.Bar5_Enabled false \
    CONFIG.Expansion_Rom_Enabled true CONFIG.Expansion_Rom_Scale Kilobytes CONFIG.Expansion_Rom_Size $rom_kib \
    CONFIG.Vendor_ID 10EE CONFIG.Device_ID 0666 CONFIG.Revision_ID 03 \
    CONFIG.Use_Class_Code_Lookup_Assistant false CONFIG.Class_Code_Base FF CONFIG.Class_Code_Sub 00 CONFIG.Class_Code_Interface 00 \
    CONFIG.MSI_Enabled false CONFIG.MSIx_Enabled false CONFIG.IntX_Generation false CONFIG.DSN_Enabled false \
    CONFIG.EXT_PCI_CFG_Space false CONFIG.AER_Enabled false CONFIG.AER_ECRC_Check_Capable false CONFIG.AER_ECRC_Gen_Capable false \
    CONFIG.cfg_mgmt_if true CONFIG.cfg_ctl_if true CONFIG.cfg_status_if true CONFIG.rcv_msg_if false CONFIG.cfg_fc_if false \
    CONFIG.err_reporting_if false CONFIG.pl_interface true CONFIG.en_ext_clk false CONFIG.en_ext_gt_common false]
set_property -dict $desired $ip
foreach {key expected} $desired {
    set actual [get_property $key $ip]
    if {$key in {CONFIG.Vendor_ID CONFIG.Device_ID CONFIG.Revision_ID CONFIG.Class_Code_Base CONFIG.Class_Code_Sub CONFIG.Class_Code_Interface}} {
        if {[scan $actual %x a] != 1 || [scan $expected %x e] != 1 || $a != $e} { error "IP property mismatch: $key ($actual != $expected)" }
    } elseif {![string equal -nocase $actual $expected]} { error "IP property mismatch: $key ($actual != $expected)" }
}
generate_target all $ip
synth_ip $ip
foreach source {svmvisor_pcie_pkg.sv svmvisor_bypass.sv svmvisor_journal.sv svmvisor_snapshot.sv svmvisor_percpu_snapshot.sv svmvisor_payload_spi.sv svmvisor_completer.sv svmvisor_tx_guard.sv svmvisor_endpoint.sv} {
    read_verilog -sv [file join $source_root rtl $source]
}
add_files -norecurse $rom_memory
set_property file_type {Memory Initialization Files} [get_files $rom_memory]
read_xdc [file join $source_root rtl svmvisor_endpoint.xdc]
set_property top svmvisor_endpoint [current_fileset]
update_compile_order -fileset sources_1
synth_design -top svmvisor_endpoint -part xc7a35tfgg484-2 -flatten_hierarchy none -generic [list FPGA_BUILD_ID=64'h$fpga_id ROM_BUILD_ID=64'h$rom_id PAYLOAD_ENABLED=$payload_enabled ROM_BYTES=$rom_bytes]
# Apply conditional SPI timing in Tcl, never as ignored XDC control flow.
if {$payload_enabled} {
    set spi_clock_pin [get_pins -quiet -hier -filter {NAME =~ *reader/spi_clk_reg/Q}]
    set spi_startup_pin [get_pins -quiet card_payload.configuration_clock/USRCCLKO]
    set spi_input_pin [get_pins -quiet {card_payload.reader/incoming_reg[0]/D}]
    set spi_output_clock [get_pins -quiet card_payload.reader/spi_mosi_reg/C]
    foreach pins [list $spi_clock_pin $spi_startup_pin $spi_input_pin $spi_output_clock] {
        if {[llength $pins] != 1} {error "Missing or ambiguous payload timing endpoint"}
    }
    # EOS is asynchronous to PCI user_clk. Only its first synchronizer stage
    # is excluded; priming cannot begin before synchronized End Of Startup.
    set_false_path -through [get_pins card_payload.configuration_clock/EOS] -to [get_pins {card_payload.reader/eos_sync_reg[0]/D}]
    set spi_clock_cell [get_cells -of_objects $spi_clock_pin]
    # Include C->Q and the actual routed net to STARTUPE2. DS181 -2 specifies
    # another 0.5..6.7ns from this primitive input to the dedicated CCLK pad.
    create_generated_clock -name payload_sck -source [get_pins $spi_clock_cell/C] -divide_by 4 $spi_startup_pin
    # Return path: STARTUPmax6.7 + boardroundtripbudget2 + ISSI tVmax8.
    # Conservatively allow flash transition immediately after STARTUPmin0.5;
    # no assumed positive board delay is used for hold.
    set_input_delay -clock payload_sck -clock_fall -max 16.7 [get_ports flash_miso]
    set_input_delay -clock payload_sck -clock_fall -min 0.5 [get_ports flash_miso]
    # The enabled negative user-edge sample is 40ns after the falling drive
    # edge. Two intervening negative edges are disabled by spi_clk/half_cycle.
    set_multicycle_path 3 -setup -end -from [get_ports flash_miso] -to $spi_input_pin
    set_multicycle_path 2 -hold -end -from [get_ports flash_miso] -to $spi_input_pin
    # SI setup: tDS2 + boardbudget2 - STARTUPmin0.5. SI hold must include
    # the latest CCLK: tDH2 + board-skewbudget1 + STARTUPmax6.7.
    set_output_delay -clock payload_sck -max 3.5 [get_ports flash_mosi]
    set_output_delay -clock payload_sck -min -9.7 [get_ports flash_mosi]
    # MOSI only changes on a falling serial drive edge, 32ns after a rising
    # edge. Moving the pessimistic hold launch by one user period (16ns)
    # remains stricter than the actual 32ns phase-qualified transition.
    set_multicycle_path 1 -hold -start -from $spi_output_clock -to [get_ports flash_mosi]
}
if {$payload_enabled && [llength [get_clocks -quiet payload_sck]] != 1} {error "Payload SCK timing constraint missing"}
set gtp [get_cells -hier -filter {REF_NAME == GTPE2_CHANNEL}]
if {[llength $gtp] != 1} { error "Expected one GTP channel" }
set_property LOC GTPE2_CHANNEL_X0Y2 $gtp
write_checkpoint -force [file join $output_dir endpoint-synth.dcp]
opt_design
place_design
phys_opt_design
route_design
write_checkpoint -force [file join $output_dir endpoint-routed.dcp]
report_utilization -file [file join $output_dir utilization.rpt]
report_timing_summary -report_unconstrained -file [file join $output_dir timing.rpt]
report_cdc -details -file [file join $output_dir cdc.rpt]
report_drc -file [file join $output_dir drc.rpt]
check_timing -verbose -file [file join $output_dir check-timing.rpt]
write_verilog -force [file join $output_dir endpoint-routed.v]
set bscan [get_cells -hier -filter {REF_NAME == BSCANE2}]
if {[llength $bscan] != 2 || [lsort [get_property JTAG_CHAIN $bscan]] ne {2 3}} { error "Expected exactly USER2 and USER3 BSCAN" }
foreach pattern {cfg_mgmt_wr_en cfg_mgmt_wr_readonly cfg_mgmt_wr_rw1c_as_rw pl_directed_link_change* pl_directed_link_width* pl_directed_link_speed pl_directed_link_auton pl_transmit_hot_rst} {
    foreach pin [get_pins pcie_core/$pattern] {
        set nets [get_nets -quiet -segments -of_objects $pin]
        if {[llength $nets] == 0} { continue }
        set drivers [get_cells -of_objects [get_pins -leaf -of_objects $nets -filter {DIRECTION == OUT}]]
        if {[llength $drivers] != 1 || [get_property REF_NAME $drivers] ne "GND"} { error "Active diagnostic write/control: $pin" }
    }
}
# Retained event storage must not acquire a PCIe-driven reset during synthesis.
set retained [get_cells -hier -filter {NAME =~ completer/journal/committed_record_reg* || NAME =~ completer/journal/head_reg* || NAME =~ completer/journal/count_reg* || NAME =~ completer/journal/rom_count_reg* || NAME =~ completer/journal/write_count_reg* || NAME =~ completer/journal/errors_reg*}]
set checked 0
foreach cell $retained {
    foreach pin [get_pins -quiet -of_objects $cell -filter {REF_PIN_NAME == R || REF_PIN_NAME == CLR}] {
        set drivers [get_cells -of_objects [get_pins -leaf -of_objects [get_nets -segments -of_objects $pin] -filter {DIRECTION == OUT}]]
        if {[llength $drivers] != 1 || [get_property REF_NAME $drivers] ne "GND"} {
            error "Retained journal storage has an active reset: $pin <- $drivers"
        }
        incr checked
    }
}
if {$checked < 256} { error "Journal retention audit did not cover committed storage" }
foreach cell [get_cells -hier -filter {NAME =~ snapshot/banks_reg* || NAME =~ snapshot/observed_reg* || NAME =~ snapshot/scan_mirror_reg* || NAME =~ snapshot/pending_body_reg* || NAME =~ snapshot/*crossing/*hsdata_ff_reg* || NAME =~ cpu_snapshot/valid_reg* || NAME =~ cpu_snapshot/errors_reg* || NAME =~ cpu_snapshot/retained* || NAME =~ cpu_snapshot/published* || NAME =~ cpu_snapshot/pending_body_reg* || NAME =~ cpu_snapshot/*mailbox* || NAME =~ cpu_snapshot/*mirror* || NAME =~ cpu_snapshot/scan_capture_buffer_reg* || NAME =~ cpu_snapshot/*crossing/*hsdata_ff_reg*}] {
    foreach pin [get_pins -quiet -of_objects $cell -filter {REF_PIN_NAME == R || REF_PIN_NAME == CLR}] {
        set drivers [get_cells -of_objects [get_pins -leaf -of_objects [get_nets -segments -of_objects $pin] -filter {DIRECTION == OUT}]]
        if {[llength $drivers] != 1 || [get_property REF_NAME $drivers] ne "GND"} { error "Snapshot storage has active reset: $pin" }
    }
}
# Structural fan-in: all user AXI TX drivers must come from the guard hierarchy.
foreach pattern {s_axis_tx_tdata* s_axis_tx_tkeep* s_axis_tx_tvalid s_axis_tx_tlast} {
    set pins [get_pins -quiet pcie_core/$pattern]
    if {[llength $pins] == 0} { error "Missing core TX pins: $pattern" }
    foreach pin $pins {
        set nets [get_nets -quiet -segments -of_objects $pin]
        # The 64-bit vendor adapter retains only tkeep[7]. Other tkeep inputs
        # are physically disconnected during optimization, never alternate TX.
        if {[llength $nets] == 0 && [string match *tkeep* $pin]} { continue }
        set drivers [get_pins -leaf -of_objects $nets -filter {DIRECTION == OUT}]
        if {[llength $drivers] != 1 || ![string match tx_guard/* [lindex $drivers 0]]} {
            error "User TX bypasses guard: $pin <- $drivers"
        }
    }
}
if {[llength [get_cells -hier -filter {REF_NAME =~ *pcileech* || REF_NAME =~ *ft601*}]] != 0} { error "Forbidden inherited module" }
set setup [get_timing_paths -delay_type max -max_paths 1]
set hold [get_timing_paths -delay_type min -max_paths 1]
if {[llength $setup] != 1 || [llength $hold] != 1 || [get_property SLACK $setup] < 0 || [get_property SLACK $hold] < 0} {
    error "Endpoint timing failed"
}
if {[llength [get_drc_violations -filter {SEVERITY == Error}]] != 0} { error "Endpoint DRC errors" }
set cdc_file [open [file join $output_dir cdc.rpt] r]
set cdc_text [read $cdc_file]
close $cdc_file
if {[regexp -line {^CDC-[0-9]+[ \t]+Critical} $cdc_text]} { error "Critical CDC findings" }
set lut_count [llength [get_cells -hier -filter {REF_NAME =~ LUT*}]]
set ram18 [llength [get_cells -hier -filter {REF_NAME =~ RAMB18*}]]
set ram36 [llength [get_cells -hier -filter {REF_NAME =~ RAMB36*}]]
if {$lut_count > 14560 || $ram18 + 2*$ram36 > 70} { error "Endpoint exceeds 70% resource budget" }
write_bitstream -force -bin_file [file join $output_dir svmvisor-endpoint.bit]
set report [open [file join $output_dir endpoint-policy.txt] w]
puts $report "guarded_user_tx_fanin=PASS"
puts $report "timing_setup_hold=PASS"
puts $report "cdc_no_critical=PASS"
puts $report "resource_headroom=PASS"
puts $report "journal_retained_storage_reset=PASS"
puts $report "user2_and_read_only_config_status=PASS"
puts $report "user3_bounded_percpu_readout=PASS"
puts $report "physical_fixture_test=NOT_RUN"
puts $report "first_light_qualification=NOT_RUN"
close $report
close_project


