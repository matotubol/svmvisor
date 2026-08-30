if {$argc != 10} {
    puts stderr "usage: build.tcl <PCIeSquirrel source> <generate|build> <patched BAR controller> <completion RTL> <option ROM RTL> <option ROM memory> <ROM KiB> <vendor ID> <device ID> <class code>"
    exit 2
}

set source_dir [file normalize [lindex $argv 0]]
set mode [lindex $argv 1]
set patched_bar_controller [file normalize [lindex $argv 2]]
set completion_rtl [file normalize [lindex $argv 3]]
set option_rom_rtl [file normalize [lindex $argv 4]]
set option_rom_memory [file normalize [lindex $argv 5]]
set rom_size_kib [lindex $argv 6]
set vendor_id [string toupper [lindex $argv 7]]
set device_id [string toupper [lindex $argv 8]]
set class_code [string toupper [lindex $argv 9]]
set project_file [file join $source_dir pcileech_squirrel pcileech_squirrel.xpr]

if {$mode ni {generate build}} {
    puts stderr "unknown mode: $mode"
    exit 2
}
if {$rom_size_kib ne "4"} {
    puts stderr "the svmvisor ROM leaf requires a 4 KiB Expansion ROM; got $rom_size_kib KiB"
    exit 2
}
if {![regexp {^[0-9A-F]{4}$} $vendor_id] || ![regexp {^[0-9A-F]{4}$} $device_id] || ![regexp {^[0-9A-F]{6}$} $class_code]} {
    puts stderr "invalid normalized PCI identity: vendor=$vendor_id device=$device_id class=$class_code"
    exit 2
}

foreach required_file [list $patched_bar_controller $completion_rtl $option_rom_rtl $option_rom_memory] {
    if {![file exists $required_file]} {
        puts stderr "required file does not exist: $required_file"
        exit 2
    }
}

cd $source_dir

if {[file exists $project_file]} {
    puts stderr "refusing to reuse existing generated project: $project_file"
    exit 2
}
source [file join $source_dir vivado_generate_project.tcl]

# Replace only the pinned upstream BAR-controller source with a generated copy
# whose Expansion ROM placeholder instantiates the svmvisor-owned ROM block.
set old_bar_controllers [get_files -quiet -of_objects [get_filesets sources_1] *pcileech_tlps128_bar_controller.sv]
if {[llength $old_bar_controllers] != 1} {
    puts stderr "expected exactly one PCILeech BAR controller, found [llength $old_bar_controllers]"
    close_project
    exit 2
}
remove_files $old_bar_controllers

foreach stale_file [get_files -quiet -of_objects [get_filesets sources_1] *svmvisor_option_rom.sv] {
    remove_files $stale_file
}
foreach stale_file [get_files -quiet -of_objects [get_filesets sources_1] *svmvisor_tlps128_bar_rdengine.sv] {
    remove_files $stale_file
}
foreach stale_file [get_files -quiet -of_objects [get_filesets sources_1] *svmvisor-dxe.mem] {
    remove_files $stale_file
}

add_files -fileset sources_1 -norecurse $patched_bar_controller
add_files -fileset sources_1 -norecurse $completion_rtl
add_files -fileset sources_1 -norecurse $option_rom_rtl
add_files -fileset sources_1 -norecurse $option_rom_memory
set_property file_type SystemVerilog [get_files -of_objects [get_filesets sources_1] *pcileech_tlps128_bar_controller.sv]
set_property file_type SystemVerilog [get_files -of_objects [get_filesets sources_1] *svmvisor_tlps128_bar_rdengine.sv]
set_property file_type SystemVerilog [get_files -of_objects [get_filesets sources_1] *svmvisor_option_rom.sv]
set_property file_type {Memory Initialization Files} [get_files -of_objects [get_filesets sources_1] *svmvisor-dxe.mem]

set pcie_ip [get_ips -quiet pcie_7x_0]
if {[llength $pcie_ip] != 1} {
    puts stderr "expected the pcie_7x_0 IP core"
    close_project
    exit 2
}
set_property -dict [list \
    CONFIG.Expansion_Rom_Enabled {true} \
    CONFIG.Expansion_Rom_Scale {Kilobytes} \
    CONFIG.Expansion_Rom_Size $rom_size_kib \
    CONFIG.Vendor_ID $vendor_id \
    CONFIG.Device_ID $device_id \
    CONFIG.Class_Code_Base [string range $class_code 0 1] \
    CONFIG.Class_Code_Sub [string range $class_code 2 3] \
    CONFIG.Class_Code_Interface [string range $class_code 4 5] \
] $pcie_ip
generate_target all $pcie_ip

foreach {property expected} [list \
    CONFIG.Expansion_Rom_Enabled true \
    CONFIG.Expansion_Rom_Scale Kilobytes \
    CONFIG.Expansion_Rom_Size $rom_size_kib \
] {
    set actual [get_property $property $pcie_ip]
    if {![string equal -nocase $actual $expected]} {
        puts stderr "PCIe IP property $property is '$actual'; expected '$expected'"
        close_project
        exit 2
    }
}
foreach {property expected} [list \
    CONFIG.Vendor_ID $vendor_id \
    CONFIG.Device_ID $device_id \
    CONFIG.Class_Code_Base [string range $class_code 0 1] \
    CONFIG.Class_Code_Sub [string range $class_code 2 3] \
    CONFIG.Class_Code_Interface [string range $class_code 4 5] \
] {
    set actual [get_property $property $pcie_ip]
    if {[scan $actual %x actual_value] != 1 || [scan $expected %x expected_value] != 1 || $actual_value != $expected_value} {
        puts stderr "PCIe IP property $property is '$actual'; expected hexadecimal '$expected'"
        close_project
        exit 2
    }
}

update_compile_order -fileset sources_1

puts "Expansion ROM BAR: [get_property CONFIG.Expansion_Rom_Size $pcie_ip] [get_property CONFIG.Expansion_Rom_Scale $pcie_ip]"
puts "Completion RTL: $completion_rtl"
puts "Expansion ROM RTL: $option_rom_rtl"
puts "Expansion ROM data: $option_rom_memory"

if {$mode eq "generate"} {
    puts "PCIeSquirrel project generation completed."
    close_project
    exit 0
}

reset_run synth_1
source [file join $source_dir vivado_build.tcl]

foreach run_name {synth_1 impl_1} {
    set run [get_runs $run_name]
    set progress [get_property PROGRESS $run]
    set status [get_property STATUS $run]
    puts "$run_name: $status ($progress)"
    if {$progress ne "100%" || [regexp -nocase {error|fail} $status]} {
        puts stderr "$run_name did not complete successfully: $status ($progress)"
        close_project
        exit 1
    }
}

set implementation_image [file join $source_dir pcileech_squirrel pcileech_squirrel.runs impl_1 pcileech_squirrel_top.bin]
if {![file exists $implementation_image] || [file size $implementation_image] == 0} {
    puts stderr "implementation did not produce a non-empty image: $implementation_image"
    close_project
    exit 1
}
close_project
