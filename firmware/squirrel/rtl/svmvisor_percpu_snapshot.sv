`timescale 1ns/1ps
// USER3/BSCANE2: AMD UG470v1.17, Table10-2 and USER-register section, pp161-163.
// Fixed diagnostic storage only: no PCIe requester, DMA, memory-reader or command path.
// User writes publish complete 19-DWORD records; transaction reset cancels staging.
// Committed records and both CDC mailboxes have configuration-only initialization.
module svmvisor_percpu_snapshot #(
    parameter logic [63:0] FPGA_BUILD_ID=0, ROM_BUILD_ID=0
)(
    input logic user_clk, board_clk, transaction_reset,
    input logic write_valid, bad_alignment,
    input logic [11:0] write_address,
    input logic [31:0] write_data,
    input logic [3:0] write_be,
    input logic drck,capture,shift,selected,tdi,
    output logic tdo
);
    wire [607:0] staged_record;
    logic [18:0] staged_valid[0:31];
    (* ram_style="distributed" *) logic [607:0] progress[0:31];
    (* ram_style="distributed" *) logic [607:0] faults[0:31];
    logic [63:0] valid=0;
    logic [31:0] errors=0;
    logic address_valid;
    logic [4:0] write_slot;
    logic [6:0] offset;
    always @* begin
        address_valid=0; write_slot=0; offset=0;
        for(integer s=0;s<32;s++) begin
            if(write_address >= 12'h600+s*80 && write_address < 12'h600+(s+1)*80) begin
                address_valid=1; write_slot=s; offset=write_address-(12'h600+s*80);
            end
        end
    end
    initial begin
        for(integer i=0;i<32;i++) staged_valid[i]=0;
        for(integer i=0;i<32;i++) begin progress[i]=0; faults[i]=0; end
    end
    // Separate DWORD banks avoid a608-bit read-modify-write feedback mux for
    // every incoming32-bit write. Each bank has one write port and one read port.
    generate for(genvar w=0;w<19;w++) begin : stage_word
        (* ram_style="distributed" *) logic [31:0] words[0:31];
        initial for(integer i=0;i<32;i++) words[i]=0;
        always @(posedge user_clk) begin
            if(!transaction_reset && !bad_alignment && write_valid && address_valid &&
                write_address[1:0]==0 && write_be==4'hf && offset==w*4)
                words[write_slot]<=write_data;
        end
        assign staged_record[w*32+:32]=words[write_slot];
    end endgenerate
    always @(posedge user_clk) begin
        if(transaction_reset || bad_alignment) begin
            for(integer i=0;i<32;i++) staged_valid[i]<=0;
            if(bad_alignment) errors[0]<=1;
        end else if(write_valid) begin
            if(!address_valid || write_address[1:0]!=0 || write_be!=4'hf) begin
                errors[0]<=1;
                if(address_valid) staged_valid[write_slot]<=0;
            end else if(offset<76) begin
                staged_valid[write_slot][offset[6:2]]<=1;
            end else begin
                staged_valid[write_slot]<=0;
                if(staged_valid[write_slot]==19'h7ffff &&
                    write_data==staged_record[31:0] && write_data!=0 &&
                    staged_record[47:40]==8'h01 && staged_record[63:49]==0) begin
                    progress[write_slot]<=staged_record;
                    valid[{1'b0,write_slot}]<=1;
                    if(staged_record[48] && !valid[{1'b1,write_slot}]) begin
                        faults[write_slot]<=staged_record;
                        valid[{1'b1,write_slot}]<=1;
                    end
                end else errors[1]<=1;
            end
        end
    end
    // Round-robin background mirroring continues without JTAG clocks. Overwritten
    // intermediate progress is intentionally coalesced; the first fault is sticky.
    logic [5:0] copy_bank=0;
    logic [646:0] user_mailbox=0;
    logic user_send=0;
    wire user_received,board_request;
    wire [646:0] board_delivery;
    always @(posedge user_clk) begin
        if(user_send) begin if(user_received) user_send<=0; end
        else if(!user_received) begin
            user_mailbox<={errors,valid[copy_bank],copy_bank,(copy_bank[5] ? faults[copy_bank[4:0]] : progress[copy_bank[4:0]])};
            user_send<=1; copy_bank<=copy_bank+1;
        end
    end
    xpm_cdc_handshake #(.WIDTH(647),.DEST_EXT_HSK(0),.DEST_SYNC_FF(3),
        .SRC_SYNC_FF(3),.INIT_SYNC_FF(1),.SIM_ASSERT_CHK(1)) record_crossing(
        .src_clk(user_clk),.src_in(user_mailbox),.src_send(user_send),.src_rcv(user_received),
        .dest_clk(board_clk),.dest_out(board_delivery),.dest_req(board_request),.dest_ack(1'b0));
    (* ram_style="distributed" *) logic [607:0] retained[0:63];
    logic [63:0] retained_valid=0;
    logic [31:0] retained_errors=0;
    initial for(integer i=0;i<64;i++) retained[i]=0;
    always @(posedge board_clk) if(board_request) begin
        retained[board_delivery[613:608]]<=board_delivery[607:0];
        retained_valid[board_delivery[613:608]]<=board_delivery[614];
        retained_errors<=board_delivery[646:615];
    end
    // The low six shifted-in bits select a bounded, read-only bank. A request is
    // accepted only after a complete 1024-bit scan; no UPDATE-derived fabric clock.
    logic [9:0] scan_index=0;
    logic [5:0] selector_bits=0,selector_mailbox=0;
    logic selector_send=0;
    wire selector_received,selector_request;
    wire [5:0] selector_delivery;
    always @(posedge drck) begin
        if(selector_send && selector_received) selector_send<=0;
        if(selected && capture) scan_index<=0;
        else if(selected && shift) begin
            scan_index<=scan_index+1;
            if(scan_index<6) selector_bits[scan_index]<=tdi;
            if(scan_index==1023 && !selector_send && !selector_received) begin
                selector_mailbox<=selector_bits; selector_send<=1;
            end
        end
    end
    xpm_cdc_handshake #(.WIDTH(6),.DEST_EXT_HSK(0),.DEST_SYNC_FF(3),
        .SRC_SYNC_FF(3),.INIT_SYNC_FF(1),.SIM_ASSERT_CHK(1)) selector_crossing(
        .src_clk(drck),.src_in(selector_mailbox),.src_send(selector_send),.src_rcv(selector_received),
        .dest_clk(board_clk),.dest_out(selector_delivery),.dest_req(selector_request),.dest_ack(1'b0));
    logic [5:0] selected_bank=0;
    always @(posedge board_clk) if(selector_request) selected_bank<=selector_delivery;
    // 128-byte frame: magic/schema, IDs, bank/valid, transport errors, opaque
    // 76-byte record, sixteen reserved bytes, CRC32 of the first124bytes.
    wire [991:0] live_body={128'b0,retained[selected_bank],retained_errors,
        25'b0,retained_valid[selected_bank],selected_bank,ROM_BUILD_ID,FPGA_BUILD_ID,
        32'h04000001,32'h55504353};
    function automatic [31:0] crc_byte(input logic [31:0] crc,input logic [7:0] value);
        logic [31:0] c; c=crc^{24'b0,value};
        for(integer i=0;i<8;i++) c=c[0] ? (c>>1)^32'hedb88320 : c>>1;
        return c;
    endfunction
    logic [991:0] pending_body=0;
    logic building=0;
    logic [6:0] byte_index=0;
    logic [31:0] crc=32'hffffffff;
    logic scan_send=0,first_send=1;
    logic [1023:0] scan_mailbox=0;
    wire scan_received,scan_request;
    wire [1023:0] scan_delivery;
    // Build directly into the acknowledged output mailbox. Retained RAM keeps
    // updating while scan clocks are absent; no second1024-bit publish bank is needed.
    always @(posedge board_clk) begin
        if(scan_send && scan_received) scan_send<=0;
        if(!building) begin
            if(!scan_send && !scan_received && (first_send || scan_mailbox[991:0]!=live_body)) begin
                pending_body<=live_body; crc<=32'hffffffff; byte_index<=0; building<=1;
            end
        end else if(byte_index==124) begin
            scan_mailbox<={~crc,pending_body}; scan_send<=1; first_send<=0; building<=0;
        end else begin
            crc<=crc_byte(crc,pending_body[byte_index*8+:8]); byte_index<=byte_index+1;
        end
    end
    xpm_cdc_handshake #(.WIDTH(1024),.DEST_EXT_HSK(0),.DEST_SYNC_FF(3),
        .SRC_SYNC_FF(3),.INIT_SYNC_FF(1),.SIM_ASSERT_CHK(1)) scan_crossing(
        .src_clk(board_clk),.src_in(scan_mailbox),.src_send(scan_send),.src_rcv(scan_received),
        .dest_clk(drck),.dest_out(scan_delivery),.dest_req(scan_request),.dest_ack(1'b0));
    logic [1023:0] scan_mirror=0,scan_capture_buffer=0;
    always @(posedge drck) begin
        if(scan_request) scan_mirror<=scan_delivery;
        if(selected && capture) scan_capture_buffer<=scan_mirror;
    end
    assign tdo=scan_capture_buffer[scan_index];
endmodule
