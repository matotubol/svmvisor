`timescale 1ns/1ps
// No PCIe reset enters this module. Both mailboxes and published banks retain
// their configuration-initialized state when PERST/user reset is asserted.
module svmvisor_snapshot #(
    parameter logic [63:0] FPGA_BUILD_ID=0, ROM_BUILD_ID=0
)(
    input logic user_clk, board_clk,
    input logic [255:0] committed_record,
    input logic [31:0] hardware_status, rom_count, write_count,
    input logic perst_n, link_up,
    input logic drck, capture, shift, selected,
    output logic tdo
);
    function automatic [31:0] crc_byte(input logic [31:0] crc,input logic [7:0] value);
        logic [31:0] c;
        c=crc ^ {24'b0,value};
        for(integer i=0;i<8;i++) c=c[0] ? (c>>1)^32'hedb88320 : c>>1;
        return c;
    endfunction
    function automatic [511:0] initial_frame();
        logic [479:0] body;
        logic [31:0] c;
        body={288'b0,ROM_BUILD_ID,FPGA_BUILD_ID,32'h02000001,32'h50414e53};
        c=32'hffffffff;
        for(integer i=0;i<60;i++) c=crc_byte(c,body[i*8+:8]);
        return {~c,body};
    endfunction
    localparam logic [511:0] INITIAL_FRAME=initial_frame();
    wire [351:0] user_live={hardware_status,write_count,rom_count,committed_record};
    logic [351:0] user_mailbox=0;
    logic user_send=0;
    wire user_received;
    logic [15:0] counter_period=0;
    // Commits and status changes bypass the counter coalescing interval.
    always @(posedge user_clk) begin
        if (counter_period != 16'hffff) counter_period <= counter_period+1;
        if (user_send) begin
            if (user_received) user_send<=0;
        end else if (!user_received &&
            (committed_record != user_mailbox[255:0] ||
             hardware_status != user_mailbox[351:320] ||
             (counter_period == 16'hffff && user_live != user_mailbox))) begin
            user_mailbox<=user_live; user_send<=1; counter_period<=0;
        end
    end
    wire [351:0] board_mailbox;
    wire board_request;
    xpm_cdc_handshake #(.WIDTH(352),.DEST_EXT_HSK(0),.DEST_SYNC_FF(3),
        .SRC_SYNC_FF(3),.INIT_SYNC_FF(1),.SIM_ASSERT_CHK(1)) journal_crossing(
        .src_clk(user_clk),.src_in(user_mailbox),.src_send(user_send),.src_rcv(user_received),
        .dest_clk(board_clk),.dest_out(board_mailbox),.dest_req(board_request),.dest_ack(1'b0));
    logic [351:0] observed=0;
    (* ASYNC_REG="TRUE" *) logic [1:0] perst_sync=3, link_sync=0;
    logic perst_seen=0;
    always @(posedge board_clk) begin
        perst_sync<={perst_sync[0],perst_n}; link_sync<={link_sync[0],link_up};
        if(!perst_sync[1]) perst_seen<=1;
        if(board_request) observed<=board_mailbox;
    end
    wire [31:0] board_status=(observed[351:320] & 32'hffffff7e) |
        {24'b0,perst_seen,6'b0,link_sync[1]};
    wire [479:0] live_body={observed[319:288],observed[287:256],board_status,
        observed[191:128],observed[223:192],observed[255:224],
        observed[31:0],observed[63:32],ROM_BUILD_ID,FPGA_BUILD_ID,
        32'h02000001,32'h50414e53};
    logic [511:0] banks[0:1];
    logic published=0, building=0;
    logic [479:0] pending_body=0;
    logic [31:0] crc=32'hffffffff;
    logic [5:0] byte_index=0;
    initial begin banks[0]=INITIAL_FRAME; banks[1]=INITIAL_FRAME; end
    always @(posedge board_clk) begin
        if(!building) begin
            if(live_body != banks[published][479:0]) begin
                pending_body<=live_body; crc<=32'hffffffff;
                byte_index<=0; building<=1;
            end
        end else if(byte_index==60) begin
            banks[!published]<={~crc,pending_body};
            published<=!published; building<=0;
        end else begin
            crc<=crc_byte(crc,pending_body[byte_index*8+:8]);
            byte_index<=byte_index+1;
        end
    end
    // A second acknowledged crossing avoids asynchronously sampling a 512-bit
    // bank on Capture-DR. Scan clocks deliver a coherent mirror; Capture-DR
    // freezes that mirror for the whole shift, even if publication continues.
    logic scan_send=0, first_send=1;
    logic [511:0] scan_mailbox=INITIAL_FRAME;
    wire scan_received, scan_request;
    wire [511:0] scan_delivery;
    always @(posedge board_clk) begin
        if(scan_send) begin if(scan_received) scan_send<=0; end
        else if(!scan_received && (first_send || scan_mailbox != banks[published])) begin
            scan_mailbox<=banks[published]; scan_send<=1; first_send<=0;
        end
    end
    xpm_cdc_handshake #(.WIDTH(512),.DEST_EXT_HSK(0),.DEST_SYNC_FF(3),
        .SRC_SYNC_FF(3),.INIT_SYNC_FF(1),.SIM_ASSERT_CHK(1)) scan_crossing(
        .src_clk(board_clk),.src_in(scan_mailbox),.src_send(scan_send),.src_rcv(scan_received),
        .dest_clk(drck),.dest_out(scan_delivery),.dest_req(scan_request),.dest_ack(1'b0));
    logic [511:0] scan_mirror=INITIAL_FRAME, scan_bits=INITIAL_FRAME;
    always @(posedge drck) begin
        if(scan_request) scan_mirror<=scan_delivery;
        if(selected && capture) scan_bits<=scan_mirror;
        else if(selected && shift) scan_bits<={1'b0,scan_bits[511:1]};
    end
    assign tdo=scan_bits[0];
endmodule
