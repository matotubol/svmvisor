`timescale 1ns/1ps
module svmvisor_endpoint #(
    parameter bit PAYLOAD_ENABLED=0,
    parameter integer ROM_BYTES=8192,
    parameter logic [63:0] FPGA_BUILD_ID=0, ROM_BUILD_ID=0
)(
    input wire clk, user_sw1_n, pcie_perst_n,
    input wire pcie_clk_p, pcie_clk_n,
    input wire [0:0] pcie_rx_p, pcie_rx_n,
    output wire [0:0] pcie_tx_p, pcie_tx_n,
    output wire user_ld1, user_ld2,
    output wire flash_cs_n, flash_mosi, flash_wp_n, flash_hold_n,
    input wire flash_miso,
    output wire ft2232_rst_n, ft601_rst_n,
    output wire ft601_wr_n, ft601_rd_n, ft601_oe_n, ft601_siwu_n, pcie_wake_n
);
    import svmvisor_pcie_pkg::*;
    wire refclk, user_clk, user_rst, link_up, core_release, bypass;
    wire [7:0] bus_number;
    wire [4:0] device_number;
    wire [2:0] function_number;
    wire [63:0] rx_data, built_data, guarded_data;
    wire [7:0] rx_keep, built_keep, guarded_keep;
    wire [21:0] rx_user;
    wire rx_valid,rx_last,rx_ready,built_valid,built_last,built_ready;
    wire guarded_valid,guarded_last,guarded_ready,token_valid,token_ready;
    wire tx_fault,rx_fault,busy,guard_idle,to_turnoff;
    wire [255:0] committed_record;
    wire [31:0] rom_count,write_count,journal_errors;
    wire [15:0] cfg_command;
    wire [5:0] ltssm;
    wire [31:0] cfg_data;
    wire cfg_done;
    reg [15:0] cfg_poll=0;
    reg rom_enabled=0;
    wire cfg_read=!user_rst && cfg_poll==0 && !cfg_done;
    always @(posedge user_clk) begin
        if(user_rst) begin cfg_poll<=0; rom_enabled<=0; end
        else if(cfg_poll!=0) cfg_poll<=cfg_poll-1;
        else if(cfg_done) begin rom_enabled<=cfg_data[0]; cfg_poll<=16'hffff; end
    end
    (* ASYNC_REG="TRUE" *) reg [1:0] status_perst_sync=3;
    reg status_perst_seen=0;
    always @(posedge user_clk) begin
        status_perst_sync<={status_perst_sync[0],pcie_perst_n};
        if(!status_perst_sync[1]) status_perst_seen<=1;
    end
    wire [31:0] hardware_status={18'b0,journal_errors[1],journal_errors[0],journal_errors[4],
        rom_enabled,cfg_command[2],cfg_command[1],status_perst_seen,ltssm,link_up};
    wire scan_drck,scan_capture,scan_shift,scan_selected,scan_tdo;
    BSCANE2 #(.JTAG_CHAIN(2)) user2(
        .DRCK(scan_drck),.CAPTURE(scan_capture),.SHIFT(scan_shift),.SEL(scan_selected),.TDO(scan_tdo),
        .RESET(),.RUNTEST(),.TCK(),.TDI(),.TMS(),.UPDATE());
    svmvisor_snapshot #(.FPGA_BUILD_ID(FPGA_BUILD_ID),.ROM_BUILD_ID(ROM_BUILD_ID)) snapshot(
        .user_clk,.board_clk(clk),.committed_record,.hardware_status,.rom_count,.write_count,
        .perst_n(pcie_perst_n),.link_up,.drck(scan_drck),.capture(scan_capture),
        .shift(scan_shift),.selected(scan_selected),.tdo(scan_tdo));
    wire diagnostic_write,diagnostic_bad_alignment;
    wire [11:0] diagnostic_address;
    wire [31:0] diagnostic_data;
    wire [3:0] diagnostic_be;
    wire cpu_drck,cpu_capture,cpu_shift,cpu_selected,cpu_tdi,cpu_tdo;
    BSCANE2 #(.JTAG_CHAIN(3)) user3(
        .DRCK(cpu_drck),.CAPTURE(cpu_capture),.SHIFT(cpu_shift),.SEL(cpu_selected),.TDI(cpu_tdi),.TDO(cpu_tdo),
        .RESET(),.RUNTEST(),.TCK(),.TMS(),.UPDATE());
    svmvisor_percpu_snapshot #(.FPGA_BUILD_ID(FPGA_BUILD_ID),.ROM_BUILD_ID(ROM_BUILD_ID)) cpu_snapshot(
        .user_clk,.board_clk(clk),.transaction_reset(user_rst),
        .write_valid(diagnostic_write),.bad_alignment(diagnostic_bad_alignment),
        .write_address(diagnostic_address),.write_data(diagnostic_data),.write_be(diagnostic_be),
        .drck(cpu_drck),.capture(cpu_capture),.shift(cpu_shift),.selected(cpu_selected),.tdi(cpu_tdi),.tdo(cpu_tdo));
    wire payload_request, payload_cancel, payload_ready, payload_response_valid, payload_response_error;
    wire [19:0] payload_address;
    wire [31:0] payload_response_data;
    generate if (PAYLOAD_ENABLED) begin : card_payload
        wire flash_clock, configuration_done;
        // CE# is forcibly high on physical reset/bypass, even if user_clk stops.
        wire controller_cs_n, controller_mosi;
        assign flash_cs_n = controller_cs_n | !pcie_perst_n | bypass;
        assign flash_mosi = controller_mosi;
        assign flash_wp_n = 1'b1; assign flash_hold_n = 1'b1;
        svmvisor_payload_spi reader(.clk(user_clk),.rst(user_rst),.cancel(payload_cancel),.configuration_done,
            .request(payload_request),.address(payload_address),.ready(payload_ready),
            .response_valid(payload_response_valid),.response_error(payload_response_error),
            .response_data(payload_response_data),.spi_clk(flash_clock),
            .spi_cs_n(controller_cs_n),.spi_mosi(controller_mosi),.spi_miso(flash_miso));
        STARTUPE2 #(.PROG_USR("FALSE")) configuration_clock(
            .CFGCLK(),.CFGMCLK(),.EOS(configuration_done),.PREQ(),.CLK(1'b0),.GSR(1'b0),.GTS(1'b0),
            .KEYCLEARB(1'b1),.PACK(1'b0),.USRCCLKO(flash_clock),.USRCCLKTS(1'b0),
            .USRDONEO(1'b1),.USRDONETS(1'b1));
    end else begin : no_card_payload
        assign flash_cs_n = 1'bz; assign flash_mosi = 1'bz;
        assign flash_wp_n = 1'bz; assign flash_hold_n = 1'bz;
        assign payload_ready = 1'b0; assign payload_response_valid = 1'b0;
        assign payload_response_error = 1'b1; assign payload_response_data = 32'hffffffff;
    end endgenerate
    read_token_t token;
    reg turnoff_pending=0;
    always @(posedge user_clk) begin
        if(user_rst) turnoff_pending <= 0;
        else if(to_turnoff) turnoff_pending <= 1;
    end
    svmvisor_bypass bypass_latch(.clk,.perst_n(pcie_perst_n),.switch_n(user_sw1_n),.bypass,.core_release);
    IBUFDS_GTE2 refclk_buffer(.O(refclk),.ODIV2(),.I(pcie_clk_p),.IB(pcie_clk_n),.CEB(1'b0));
    assign ft2232_rst_n=1'b1;
    assign ft601_rst_n=1'b0;
    assign ft601_wr_n=1'b1; assign ft601_rd_n=1'b1;
    assign ft601_oe_n=1'b1; assign ft601_siwu_n=1'b1; assign pcie_wake_n=1'b1;
    assign user_ld1=bypass;
    assign user_ld2=link_up | tx_fault | rx_fault;
    svmvisor_completer #(.FPGA_BUILD_ID(FPGA_BUILD_ID),.ROM_BUILD_ID(ROM_BUILD_ID),
        .PAYLOAD_ENABLED(PAYLOAD_ENABLED),.ROM_BYTES(ROM_BYTES)) completer(.clk(user_clk),.rst(user_rst),
        .hardware_status,.committed_record,.rom_count,.write_count,.journal_errors,
        .diagnostic_write,.diagnostic_bad_alignment,.diagnostic_address,.diagnostic_data,.diagnostic_be,
        .completer_id({bus_number,device_number,function_number}),
        .rx_data,.rx_keep,.rx_user,.rx_valid,.rx_last,.rx_ready,
        .token,.token_valid,.token_ready,.tx_data(built_data),.tx_keep(built_keep),
        .tx_valid(built_valid),.tx_last(built_last),.tx_ready(built_ready),.tx_fault,.rx_fault,.busy,
        .payload_request,.payload_cancel,.payload_address,.payload_ready,
        .payload_response_valid,.payload_response_error,.payload_response_data);
    (* KEEP_HIERARCHY = "yes" *) svmvisor_tx_guard tx_guard(.clk(user_clk),.rst(user_rst),
        .token,.token_valid,.token_ready,.in_data(built_data),.in_keep(built_keep),
        .in_valid(built_valid),.in_last(built_last),.in_ready(built_ready),
        .out_data(guarded_data),.out_keep(guarded_keep),.out_valid(guarded_valid),
        .out_last(guarded_last),.out_ready(guarded_ready),.violation(tx_fault),.idle(guard_idle));
    // PG054: configuration replies/link protocol remain inside the vendor core.
    // There is exactly one user TX source, the store-and-forward guard above.
    (* KEEP_HIERARCHY = "yes" *) svmvisor_pcie_core pcie_core(
        .pci_exp_txp(pcie_tx_p),.pci_exp_txn(pcie_tx_n),.pci_exp_rxp(pcie_rx_p),.pci_exp_rxn(pcie_rx_n),
        .sys_clk(refclk),.sys_rst_n(core_release),.user_clk_out(user_clk),.user_reset_out(user_rst),.user_lnk_up(link_up),
        .s_axis_tx_tdata(guarded_data),.s_axis_tx_tkeep(guarded_keep),.s_axis_tx_tvalid(guarded_valid),
        .s_axis_tx_tlast(guarded_last),.s_axis_tx_tready(guarded_ready),.s_axis_tx_tuser(4'b0),
        .m_axis_rx_tdata(rx_data),.m_axis_rx_tkeep(rx_keep),.m_axis_rx_tvalid(rx_valid),
        .m_axis_rx_tlast(rx_last),.m_axis_rx_tready(rx_ready),.m_axis_rx_tuser(rx_user),
        .cfg_bus_number(bus_number),.cfg_device_number(device_number),.cfg_function_number(function_number),
        .cfg_command,.pl_ltssm_state(ltssm),
        .cfg_mgmt_do(cfg_data),.cfg_mgmt_rd_wr_done(cfg_done),.cfg_mgmt_rd_en(cfg_read),
        .cfg_mgmt_dwaddr(10'h00c),.cfg_mgmt_byte_en(4'hf),.cfg_mgmt_di(32'b0),
        .cfg_mgmt_wr_en(1'b0),.cfg_mgmt_wr_readonly(1'b0),.cfg_mgmt_wr_rw1c_as_rw(1'b0),
        .pl_directed_link_change(2'b0),.pl_directed_link_width(2'b0),.pl_directed_link_speed(1'b0),
        .pl_directed_link_auton(1'b0),.pl_upstream_prefer_deemph(1'b0),.pl_transmit_hot_rst(1'b0),
        .pl_downstream_deemph_source(1'b0),
        .tx_cfg_gnt(1'b1),.rx_np_ok(1'b1),.rx_np_req(1'b1),
        .cfg_trn_pending(busy | !guard_idle),.cfg_to_turnoff(to_turnoff),
        .cfg_turnoff_ok(turnoff_pending && !busy && guard_idle),
        .cfg_pm_halt_aspm_l0s(1'b0),.cfg_pm_halt_aspm_l1(1'b0),.cfg_pm_force_state_en(1'b0),
        .cfg_pm_force_state(2'b0),.cfg_pm_wake(1'b0),.cfg_pm_send_pme_to(1'b0),
        .cfg_dsn(64'b0),.cfg_ds_bus_number(8'b0),.cfg_ds_device_number(5'b0),.cfg_ds_function_number(3'b0),
        .cfg_interrupt(1'b0),.cfg_interrupt_assert(1'b0),.cfg_interrupt_di(8'b0),
        .cfg_interrupt_stat(1'b0),.cfg_pciecap_interrupt_msgnum(5'b0),
        .pcie_drp_clk(clk),.pcie_drp_en(1'b0),.pcie_drp_we(1'b0),.pcie_drp_addr(9'b0),.pcie_drp_di(16'b0)
    );
endmodule
