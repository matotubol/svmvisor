`timescale 1ns/1ps
// Store-and-forward: nothing reaches the IP until the entire completion has
// matched the outstanding inbound token, length, requester, tag and byte count.
module svmvisor_tx_guard (
    input logic clk, rst,
    input svmvisor_pcie_pkg::read_token_t token,
    input logic token_valid,
    output logic token_ready,
    input logic [63:0] in_data,
    input logic [7:0] in_keep,
    input logic in_valid, in_last,
    output logic in_ready,
    output logic [63:0] out_data,
    output logic [7:0] out_keep,
    output logic out_valid, out_last,
    input logic out_ready,
    output logic violation,
    output logic idle
);
    import svmvisor_pcie_pkg::*;
    typedef enum logic [1:0] {CAPTURE, CHECK, SEND} state_t;
    state_t state;
    read_token_t current;
    logic active, bad;
    logic [63:0] packets [0:9];
    logic [4:0] captured, sent;
    logic [5:0] words_received;
    wire [5:0] expected_words = current.ur ? 6'd3 : 6'd3 + chunk_words(current);
    assign token_ready = !active && state == CAPTURE && captured == 0 && !violation;
    assign idle = token_ready;
    assign in_ready = state == CAPTURE;
    assign out_valid = state == SEND;
    assign out_data = packets[sent];
    assign out_last = sent == captured-1;
    assign out_keep = out_last && expected_words[0] ? 8'h0f : 8'hff;
    always @(posedge clk) begin
        if (rst) begin
            state <= CAPTURE; active <= 0; bad <= 0; violation <= 0;
            captured <= 0; sent <= 0; words_received <= 0; current <= '0;
        end else begin
            if (token_valid && token_ready) begin current <= token; active <= 1; end
            case (state)
                CAPTURE: if (in_valid && in_ready) begin
                    if (captured < 10) packets[captured] <= in_data;
                    if (!active || captured >= 10 ||
                        (!in_last && in_keep != 8'hff) ||
                        (in_last && in_keep != 8'hff && in_keep != 8'h0f)) bad <= 1;
                    if (captured < 11) captured <= captured + 1;
                    if (words_received < 40) words_received <= words_received + (in_keep == 8'h0f ? 1 : 2);
                    if (in_last) state <= CHECK;
                end
                CHECK: begin
                    if (bad || !active || words_received != expected_words ||
                        packets[0][31:0] != completion_h0(current) ||
                        packets[0][63:32] != completion_h1(current) ||
                        packets[1][31:0] != completion_h2(current)) begin
                        violation <= 1; active <= 0; state <= CAPTURE;
                        captured <= 0; words_received <= 0; bad <= 0;
                    end else begin sent <= 0; state <= SEND; end
                end
                SEND: if (out_ready) begin
                    if (out_last) begin
                        if (current.ur || current.words == chunk_words(current)) active <= 0;
                        else current <= advance_token(current);
                        state <= CAPTURE; captured <= 0; words_received <= 0; bad <= 0;
                    end else sent <= sent + 1;
                end
                default: begin violation <= 1; state <= CAPTURE; active <= 0; end
            endcase
        end
    end
endmodule
