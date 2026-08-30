`timescale 1ns / 1ps

// Behavioral models for the two FIFO Generator instances used by the BAR
// read engine. These are simulation-only and intentionally model standard
// (registered-output) FIFO timing.
module fifo_74_74_clk1_bar_rd1(
    input              srst,
    input              clk,
    input              wr_en,
    input [73:0]       din,
    output             full,
    input              rd_en,
    output reg [73:0]  dout,
    output             empty,
    output reg         valid
);
    localparam integer DEPTH = 16;
    reg [73:0] storage [0:DEPTH-1];
    reg [3:0] write_pointer;
    reg [3:0] read_pointer;
    reg [4:0] count;
    wire write_accepted = wr_en && !full;
    wire read_accepted = rd_en && !empty;

    assign full = (count == DEPTH);
    assign empty = (count == 0);

    always @(posedge clk) begin
        if (srst) begin
            write_pointer <= 4'd0;
            read_pointer <= 4'd0;
            count <= 5'd0;
            dout <= 74'd0;
            valid <= 1'b0;
        end else begin
            valid <= read_accepted;
            if (write_accepted) begin
                storage[write_pointer] <= din;
                write_pointer <= write_pointer + 1'b1;
            end
            if (read_accepted) begin
                dout <= storage[read_pointer];
                read_pointer <= read_pointer + 1'b1;
            end

            case ({write_accepted, read_accepted})
                2'b10: count <= count + 1'b1;
                2'b01: count <= count - 1'b1;
                default: count <= count;
            endcase
        end
    end
endmodule

module fifo_134_134_clk1_bar_rdrsp(
    input               srst,
    input               clk,
    input [133:0]       din,
    input               wr_en,
    input               rd_en,
    output reg [133:0]  dout,
    output              full,
    output              empty,
    output              prog_empty,
    output reg          valid
);
    localparam integer DEPTH = 16;
    localparam integer PROG_EMPTY_THRESHOLD = 12;
    reg [133:0] storage [0:DEPTH-1];
    reg [3:0] write_pointer;
    reg [3:0] read_pointer;
    reg [4:0] count;
    wire write_accepted = wr_en && !full;
    wire read_accepted = rd_en && !empty;

    assign full = (count == DEPTH);
    assign empty = (count == 0);
    assign prog_empty = (count < PROG_EMPTY_THRESHOLD);

    always @(posedge clk) begin
        if (srst) begin
            write_pointer <= 4'd0;
            read_pointer <= 4'd0;
            count <= 5'd0;
            dout <= 134'd0;
            valid <= 1'b0;
        end else begin
            valid <= read_accepted;
            if (write_accepted) begin
                storage[write_pointer] <= din;
                write_pointer <= write_pointer + 1'b1;
            end
            if (read_accepted) begin
                dout <= storage[read_pointer];
                read_pointer <= read_pointer + 1'b1;
            end

            case ({write_accepted, read_accepted})
                2'b10: count <= count + 1'b1;
                2'b01: count <= count - 1'b1;
                default: count <= count;
            endcase
        end
    end
endmodule
