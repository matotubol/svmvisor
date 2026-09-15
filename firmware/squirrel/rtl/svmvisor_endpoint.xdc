# Pinned Squirrel board mapping; no upstream controller or timing exceptions.
set_property PACKAGE_PIN H4 [get_ports clk]
set_property PACKAGE_PIN AB3 [get_ports user_sw1_n]
set_property PACKAGE_PIN B13 [get_ports pcie_perst_n]
set_property PACKAGE_PIN Y6 [get_ports user_ld1]
set_property PACKAGE_PIN AB5 [get_ports user_ld2]
set_property PACKAGE_PIN F21 [get_ports ft2232_rst_n]
set_property PACKAGE_PIN Y9 [get_ports ft601_rst_n]
set_property PACKAGE_PIN AB7 [get_ports ft601_wr_n]
set_property PACKAGE_PIN AA6 [get_ports ft601_rd_n]
set_property PACKAGE_PIN AB6 [get_ports ft601_oe_n]
set_property PACKAGE_PIN Y8 [get_ports ft601_siwu_n]
set_property PACKAGE_PIN A14 [get_ports pcie_wake_n]
set_property IOSTANDARD LVCMOS33 [get_ports {clk user_sw1_n pcie_perst_n user_ld1 user_ld2 ft2232_rst_n ft601_rst_n ft601_wr_n ft601_rd_n ft601_oe_n ft601_siwu_n pcie_wake_n}]
set_property PACKAGE_PIN A10 [get_ports {pcie_rx_n[0]}]
set_property PACKAGE_PIN B10 [get_ports {pcie_rx_p[0]}]
set_property PACKAGE_PIN A6 [get_ports {pcie_tx_n[0]}]
set_property PACKAGE_PIN B6 [get_ports {pcie_tx_p[0]}]
set_property PACKAGE_PIN E6 [get_ports pcie_clk_n]
set_property PACKAGE_PIN F6 [get_ports pcie_clk_p]
create_clock -period 10.000 -name board_clk [get_ports clk]
create_clock -period 10.000 -name pcie_refclk [get_ports pcie_clk_p]
# Reader is limited to 1 MHz. XPM supplies mailbox CDC constraints; no broad
# clock-group exception cuts the journal or board-clock publication logic.
create_clock -period 1000.000 -name jtag_drck [get_pins user2/DRCK]
create_clock -period 1000.000 -name cpu_jtag_drck [get_pins user3/DRCK]
set_false_path -through [get_pins {user3/CAPTURE user3/SHIFT user3/SEL}]
# Hard TAP TDI is stable for the 1MHz scan half-cycle. Constrain only its
# fabric route to the first selector register to100ns (400ns margin).
set_max_delay -datapath_only 100 -from [get_pins user3/TDI] -to [get_pins -hier -filter {NAME =~ cpu_snapshot/selector_bits_reg*/D}]
set_min_delay 0 -from [get_pins user3/TDI] -to [get_pins -hier -filter {NAME =~ cpu_snapshot/selector_bits_reg*/D}]
# UG470p164: BSCAN registers TDO on fallingTCK; DRCK-driven shift data
# gets a half-cycle. Budget100ns of the500ns half-cycle for its fabric route.
set_max_delay -datapath_only 100 -from [get_pins -hier -filter {NAME =~ cpu_snapshot/scan_capture_buffer_reg*/Q}] -to [get_pins user3/TDO]
set_min_delay 0 -from [get_pins -hier -filter {NAME =~ cpu_snapshot/scan_capture_buffer_reg*/Q}] -to [get_pins user3/TDO]
# Hard TAP control outputs are not fabric timing startpoints. These three
# controls are generated with DRCK by BSCAN, and only qualify the scan register.
set_false_path -through [get_pins {user2/CAPTURE user2/SHIFT user2/SEL}]
# Link status is asynchronous to the board clock; only the first ASYNC_REG
# stage is excluded. The second stage and all consumers remain timed.
set_false_path -to [get_pins {snapshot/link_sync_reg[0]/D}]
set_property CFGBVS Vcco [current_design]
set_property CONFIG_VOLTAGE 3.3 [current_design]
set_property BITSTREAM.CONFIG.SPI_BUSWIDTH 4 [current_design]
set_property BITSTREAM.CONFIG.SPI_FALL_EDGE YES [current_design]
set_property BITSTREAM.CONFIG.CONFIGRATE 66 [current_design]
set_property BITSTREAM.GENERAL.COMPRESS TRUE [current_design]
set_property BITSTREAM.CONFIG.UNUSEDPIN Pullnone [current_design]
# Asynchronous physical switch only enters the first synchronizer stage.
set_false_path -from [get_ports user_sw1_n] -to [get_pins -hier -filter {NAME =~ *switch_sync_reg[0]/D}]
# PERST# is the platform's asynchronous reset, not a timed data input.
set_false_path -from [get_ports pcie_perst_n]
# A single registered, monotonic startup reset release drives only the vendor
# sys_rst_n network. PG054 defines this as an asynchronous reset; its core
# reset synchronizers release each internal domain. Unlike a clock-group cut,
# this exception cannot hide application data crossings between the clocks.
set_false_path -from [get_pins bypass_latch/core_release_reg/C]
# Status LEDs and static output controls have no external synchronous receiver.
set_false_path -to [get_ports {user_ld1 user_ld2 ft2232_rst_n ft601_rst_n ft601_wr_n ft601_rd_n ft601_oe_n ft601_siwu_n pcie_wake_n}]
# Dedicated configuration functions verified against AMD Vivado's
# xc7a35tfgg484-2 package database (PIN_FUNC), not borrowed board GPIOs:
# T19 FCS_B, P22 D00_MOSI, R22 D01_DIN, P21 D02, R21 D03.
# CCLK is the dedicated L12 pin reached only through STARTUPE2.
set_property PACKAGE_PIN T19 [get_ports flash_cs_n]
set_property PACKAGE_PIN P22 [get_ports flash_mosi]
set_property PACKAGE_PIN R22 [get_ports flash_miso]
set_property PACKAGE_PIN P21 [get_ports flash_wp_n]
set_property PACKAGE_PIN R21 [get_ports flash_hold_n]
set_property IOSTANDARD LVCMOS33 [get_ports {flash_cs_n flash_mosi flash_miso flash_wp_n flash_hold_n}]
set_property SLEW FAST [get_ports {flash_cs_n flash_mosi}]
set_property DRIVE 8 [get_ports {flash_cs_n flash_mosi flash_wp_n flash_hold_n}]
# SPI is source-synchronous mode 0, 15.625 MHz. Read data changes on the
# preceding falling SCK edge; no broad false path masks the MISO sampling path.
# IS25LP256D Rev.A section9.6 tV<=8ns, DS181 STARTUP<=6.7ns;
# endpoint.tcl includes actual clock routing plus a 0..2ns board roundtrip budget.
# Conditional generated-clock and SPI I/O delays are applied after synthesis
# by endpoint.tcl (Vivado XDC does not support conditional Tcl commands).
set_false_path -to [get_ports {flash_cs_n flash_wp_n flash_hold_n}]
