`timescale 1ns/1ps
// Configuration-initialized storage: transaction reset invalidates staging only.
// No PERST/user reset clears committed evidence, counters, or sticky faults.
module svmvisor_journal #(
    parameter logic [63:0] FPGA_BUILD_ID = 0,
    parameter logic [63:0] ROM_BUILD_ID = 0,
    parameter bit SNAPSHOT_CAPABLE = 0
)(
    input logic clk, transaction_reset,
    input logic write_valid,
    input logic [11:0] write_address,
    input logic [31:0] write_data,
    input logic [3:0] write_be,
    input logic rom_read, bar_write_packet, rx_fault, tx_fault, bad_alignment,
    input logic [31:0] hardware_status,
    input logic [11:0] read_address,
    output logic [31:0] read_data,
    output logic [255:0] committed_record,
    output logic commit_pulse,
    output logic [31:0] rom_count, write_count, errors
);
    logic [255:0] staging = 0;
    logic [7:0] staged_valid = 0;
    (* ram_style = "distributed" *) logic [255:0] ring [0:31];
    logic [4:0] head = 0;
    logic [6:0] count = 0;
    // Sticky errors: bad target, byte enable/alignment, bad/incomplete commit,
    // malformed RX, TX policy violation. They cannot be cleared through BAR0.
    initial begin
        committed_record=0; commit_pulse=0; rom_count=0; write_count=0; errors=0;
        for(integer i=0;i<32;i++) ring[i]=0;
    end
    always @(posedge clk) begin
        commit_pulse <= 0;
        if (rx_fault) errors[3] <= 1;
        if (tx_fault) errors[4] <= 1;
        if (bad_alignment) errors[1] <= 1;
        if (transaction_reset || bad_alignment) staged_valid <= 0;
        else begin
            if (rom_read && rom_count != 32'hffffffff) rom_count <= rom_count+1;
            if (bar_write_packet && write_count != 32'hffffffff) write_count <= write_count+1;
            if (write_valid) begin
                if (write_address[1:0] != 0 || write_be != 4'hf) begin
                    errors[1] <= 1;
                    // Never let an earlier staged value substitute for a bad write.
                    staged_valid <= 0;
                end else if (write_address >= 12'h040 && write_address < 12'h060) begin
                    staging[write_address[4:2]*32+:32] <= write_data;
                    staged_valid[write_address[4:2]] <= 1;
                end else if (write_address == 12'h060) begin
                    staged_valid <= 0;
                    if (staged_valid == 8'hff && write_data == staging[31:0]) begin
                        ring[head] <= staging;
                        committed_record <= staging;
                        head <= head+1;
                        if (count != 32) count <= count+1;
                        commit_pulse <= 1;
                    end else errors[2] <= 1;
                end else begin errors[0] <= 1; staged_valid <= 0; end
            end
        end
    end
    always @* begin
        read_data=0;
        if (read_address[1:0] == 0) begin
            case(read_address)
                12'h000: read_data=32'h4a4d5653;
                12'h004: read_data={14'b0,SNAPSHOT_CAPABLE,SNAPSHOT_CAPABLE,16'h0001};
                12'h008: read_data=FPGA_BUILD_ID[31:0];
                12'h00c: read_data=FPGA_BUILD_ID[63:32];
                12'h010: read_data=ROM_BUILD_ID[31:0];
                12'h014: read_data=ROM_BUILD_ID[63:32];
                12'h018: read_data=hardware_status | {18'b0,errors[1],errors[0],errors[4],11'b0};
                12'h01c: read_data=rom_count;
                12'h020: read_data=write_count;
                12'h024: read_data=errors;
                12'h028: read_data={9'b0,count,11'b0,head};
                12'h02c: read_data=committed_record[31:0];
                default: begin
                    if (read_address >= 12'h040 && read_address < 12'h060)
                        read_data=staging[read_address[4:2]*32+:32];
                    else if (read_address >= 12'h080 && read_address < 12'h0a0)
                        read_data=committed_record[read_address[4:2]*32+:32];
                    else if (read_address >= 12'h100 && read_address < 12'h500)
                        read_data=ring[(read_address-12'h100)>>5][read_address[4:2]*32+:32];
                end
            endcase
        end
    end
endmodule
