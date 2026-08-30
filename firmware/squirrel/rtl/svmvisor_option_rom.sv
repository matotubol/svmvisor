`timescale 1ns / 1ps

// Read-only, 4 KiB Expansion ROM implementation for PCILeech BAR slot 6.
//
// The surrounding PCILeech read engine turns these DWORD responses into PCIe
// Completion-with-Data TLPs. Its BAR interface uses little-endian numeric
// DWORDs and performs the final conversion to PCIe wire byte order.
module svmvisor_option_rom #(
    parameter integer ROM_WORDS = 1024,
    parameter string  INIT_FILE = "svmvisor-dxe.mem"
)(
    input               rst,
    input               clk,
    // Incoming BAR writes are intentionally ignored: Expansion ROM is read-only.
    input [31:0]        wr_addr,
    input [3:0]         wr_be,
    input [31:0]        wr_data,
    input               wr_valid,
    // Incoming BAR reads.
    input [87:0]        rd_req_ctx,
    input [31:0]        rd_req_addr,
    input               rd_req_valid,
    // Outgoing BAR read data; latency is exactly two clocks.
    output reg [87:0]   rd_rsp_ctx,
    output reg [31:0]   rd_rsp_data,
    output reg          rd_rsp_valid
);

    (* rom_style = "block" *) reg [31:0] rom [0:ROM_WORDS-1];
    reg [31:0] rom_data;
    reg [87:0] rd_req_ctx_q;
    reg        rd_req_valid_q;
    integer    word_index;

    initial begin
        for (word_index = 0; word_index < ROM_WORDS; word_index = word_index + 1)
            rom[word_index] = 32'hffffffff;
        if (INIT_FILE != "")
            $readmemh(INIT_FILE, rom);
    end

    always @(posedge clk) begin
        if (rst) begin
            rom_data       <= 32'hffffffff;
            rd_req_ctx_q   <= 88'b0;
            rd_req_valid_q <= 1'b0;
            rd_rsp_ctx     <= 88'b0;
            rd_rsp_data    <= 32'hffffffff;
            rd_rsp_valid   <= 1'b0;
        end else begin
            // The core only asserts BAR 6 for offsets inside the configured
            // 4 KiB aperture. Keeping all twelve offset bits prevents padding
            // reads from wrapping into the start of the image.
            rom_data       <= rom[rd_req_addr[11:2]];
            rd_req_ctx_q   <= rd_req_ctx;
            rd_req_valid_q <= rd_req_valid;
            rd_rsp_ctx     <= rd_req_ctx_q;
            rd_rsp_data    <= rom_data;
            rd_rsp_valid   <= rd_req_valid_q;
        end
    end

endmodule
