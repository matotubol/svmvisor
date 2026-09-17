`timescale 1ns/1ps
// One outstanding inbound request; full-frame validation precedes BAR writes.
// 64-byte completion boundaries, 128-byte maximum inbound write payload.
// Reference: PG054, Transaction Interface and 64-bit RX/TX timing diagrams.
module svmvisor_completer #(
    parameter bit PAYLOAD_ENABLED = 0,
    parameter integer ROM_BYTES = 8192,
    parameter integer PAYLOAD_TIMEOUT_CYCLES = 384,
    parameter ROM_FILE = "svmvisor-dxe.mem",
    parameter logic [63:0] FPGA_BUILD_ID=0, ROM_BUILD_ID=0
)(
    input logic clk, rst,
    input logic [31:0] hardware_status,
    output logic [255:0] committed_record,
    output logic [31:0] rom_count, write_count, journal_errors,
    output logic diagnostic_write, diagnostic_bad_alignment,
    output logic [11:0] diagnostic_address,
    output logic [31:0] diagnostic_data,
    output logic [3:0] diagnostic_be,
    input logic [15:0] completer_id,
    input logic [63:0] rx_data,
    input logic [7:0] rx_keep,
    input logic [21:0] rx_user,
    input logic rx_valid, rx_last,
    output logic rx_ready,
    output svmvisor_pcie_pkg::read_token_t token,
    output logic token_valid,
    input logic token_ready,
    output logic [63:0] tx_data,
    output logic [7:0] tx_keep,
    output logic tx_valid, tx_last,
    input logic tx_ready,
    input logic tx_fault,
    output logic rx_fault,
    output logic busy,
    output logic payload_request, payload_cancel,
    output logic [19:0] payload_address,
    input logic payload_ready, payload_response_valid, payload_response_error,
    input logic [31:0] payload_response_data
);
    import svmvisor_pcie_pkg::*;
    typedef enum logic [3:0] {RX, DECODE, TOKEN, HEADER, READ_WORD, STORE_WORD, TX,
        WAIT_RETIRE, WRITE_WORD, PAYLOAD_WAIT} state_t;
    state_t state;
    logic [31:0] packet [0:35];
    logic [31:0] reply [0:19];
    (* rom_style = "block" *) logic [31:0] rom [0:ROM_BYTES/4-1];
    wire [31:0] journal_data;
    logic [31:0] read_data;
    logic [6:0] received;
    logic [5:0] index, tx_index;
    logic [7:0] bar;
    logic bad, read_rom, read_payload;
    logic [$clog2(ROM_BYTES)-13:0] rom_page;
    logic [7:0] payload_page;
    logic [15:0] payload_timer;
    logic payload_failed, payload_sent;

    assign payload_request = PAYLOAD_ENABLED && state == PAYLOAD_WAIT && !payload_sent && !payload_failed && !rst;
    assign payload_cancel = rst || (state == PAYLOAD_WAIT && payload_timer == PAYLOAD_TIMEOUT_CYCLES-1);

    read_token_t current;
    integer k;
    initial begin
        for (integer r=0; r<ROM_BYTES/4; r=r+1) rom[r] = 32'hffffffff;
        if (ROM_FILE != "") $readmemh(ROM_FILE,rom);
    end
    wire [31:0] h0 = packet[0];
    wire [31:0] h1 = packet[1];
    wire four_dw = h0[29];
    wire has_data = h0[30];
    wire [10:0] length_dw = h0[9:0] == 0 ? 11'd1024 : {1'b0,h0[9:0]};
    wire [31:0] addr = four_dw ? packet[3] : packet[2];
    wire [1:0] first = first_byte(h1[3:0]);
    wire [1:0] last = last_byte(length_dw == 1 ? h1[3:0] : h1[7:4]);
    wire [12:0] byte_count = length_dw == 1 && h1[3:0] == 0 ? 13'd1 :
        (({2'b0,length_dw}-1)<<2) + {11'b0,last} + 13'd1 - {11'b0,first};
    wire [12:0] end_address = {1'b0,addr[11:0]} + ({2'b0,length_dw}<<2);
    wire [11:0] word_addr = current.address + {4'b0,index,2'b0};
    assign payload_address = {payload_page,word_addr};
    wire [3:0] write_be = index == 0 ? h1[3:0] :
        index == length_dw-1 ? h1[7:4] : 4'hf;
    wire [31:0] write_data = swap_bytes(packet[(four_dw ? 4 : 3)+index]);
    wire [5:0] reply_words = current.ur ? 6'd3 : 6'd3 + chunk_words(current);
    assign rx_ready = state == RX && !tx_fault;
    assign busy = state != RX || received != 0;
    assign token = current;
    assign token_valid = state == TOKEN;
    assign tx_valid = state == TX;
    assign tx_data = {reply[tx_index+1],reply[tx_index]};
    assign tx_last = tx_index + 2 >= reply_words;
    assign tx_keep = tx_last && reply_words[0] ? 8'h0f : 8'hff;
    assign diagnostic_write=state==WRITE_WORD && !rst && word_addr>=12'h600;
    assign diagnostic_address=word_addr;
    assign diagnostic_data=write_data;
    assign diagnostic_be=write_be;
    assign diagnostic_bad_alignment=state==DECODE && received>=(four_dw ? 4 : 3) &&
        bar==8'h01 && has_data && addr[1:0]!=0 && !rst;
    svmvisor_journal #(.FPGA_BUILD_ID(FPGA_BUILD_ID),.ROM_BUILD_ID(ROM_BUILD_ID),.SNAPSHOT_CAPABLE(1)) journal(
        .clk, .transaction_reset(rst),
        .write_valid(state == WRITE_WORD && !rst && word_addr<12'h600), .write_address(word_addr),
        .write_data, .write_be,
        .rom_read(state == TOKEN && token_ready && read_rom && !rst),
        .bar_write_packet(state == WRITE_WORD && index == 0 && !rst),
        .rx_fault, .tx_fault,
        .bad_alignment(state == DECODE && received >= (four_dw ? 4 : 3) &&
            bar == 8'h01 && has_data && addr[1:0] != 0 && !rst),
        .hardware_status,
        .read_address(word_addr), .read_data(journal_data),
        .committed_record, .commit_pulse(), .rom_count, .write_count, .errors(journal_errors));
    always @(posedge clk) begin
        if (rst) begin
            state <= RX; received <= 0; bad <= 0; bar <= 0; index <= 0;
            tx_index <= 0; current <= '0; rx_fault <= 0;
            read_rom <= 0; rom_page <= 0; read_data <= 0; read_payload <= 0;
            payload_page <= 0; payload_timer <= 0; payload_failed <= 0; payload_sent <= 0;
        end else case (state)
            RX: if (rx_valid && rx_ready) begin
                if (received == 0) bar <= rx_user[9:2];
                if (received < 36) packet[received] <= rx_data[31:0];
                if (received < 35 && rx_keep == 8'hff) packet[received+1] <= rx_data[63:32];
                if (received < 38) received <= received + (rx_keep == 8'h0f ? 1 : 2);
                if (received >= 36 || rx_user[1:0] != 0 ||
                    (!rx_last && rx_keep != 8'hff) ||
                    (rx_last && rx_keep != 8'hff && rx_keep != 8'h0f)) bad <= 1;
                if (rx_last) state <= DECODE;
            end
            DECODE: begin
                // Reject malformed, poisoned, translated, or ECRC-bearing frames.
                // This first candidate advertises no ECRC/ATS/atomic capability.
                if (bad || received < 3 || h0[31] || h0[15:14] != 0 || h0[11:10] != 0 ||
                    !(bar == 8'h01 || bar == 8'h40 || (PAYLOAD_ENABLED && bar == 8'h02)) ||
                    received != (four_dw ? 4 : 3) + (has_data ? length_dw : 0) ||
                    (has_data && length_dw > 32) || addr[1:0] != 0 || end_address > 4096 ||
                    (length_dw == 1 && h1[7:4] != 0) ||
                    (length_dw > 1 && (h1[3:0] == 0 || h1[7:4] == 0))) begin
                    rx_fault <= 1; state <= RX; received <= 0; bad <= 0;
                end else if (h0[28:24] == 0 && has_data) begin
                    // Posted writes never originate a completion. ROM/payload writes ignored.
                    current.address <= addr[11:0]; index <= 0;
                    if (bar == 8'h01) state <= WRITE_WORD;
                    else begin state <= RX; received <= 0; end
                end else if (!has_data && (h0[28:24] == 0 || h0[28:24] == 1 || h0[28:24] == 2)) begin
                    current.completer <= completer_id;
                    current.requester <= h1[31:16]; current.tag <= h1[15:8];
                    current.tc <= h0[22:20]; current.attr <= {h0[18],h0[13:12]};
                    current.address <= addr[11:0]; current.words <= length_dw;
                    current.bytes_left <= byte_count; current.first_offset <= first;
                    // Limit the entire payload request, not just each completion,
                    // to 32 bytes so serial service stays below 40 us.
                    current.ur <= h0[28:24] != 0 || (bar == 8'h02 && length_dw > 8);
                    read_rom <= bar == 8'h40;
                    read_payload <= PAYLOAD_ENABLED && bar == 8'h02;
                    payload_page <= addr[19:12]; payload_failed <= 0;
                    // Requests cannot cross a PCIe 4 KiB boundary. Retain the
                    // ROM half independently of the guard's page-local token.
                    rom_page <= addr[$clog2(ROM_BYTES)-1:12];
                    state <= TOKEN;
                end else begin rx_fault <= 1; state <= RX; received <= 0; end
            end
            TOKEN: if (token_ready) state <= HEADER;
            HEADER: begin
                reply[0] <= completion_h0(current); reply[1] <= completion_h1(current);
                reply[2] <= completion_h2(current); reply[19] <= 0;
                index <= 0; tx_index <= 0;
                state <= current.ur ? TX : READ_WORD;
            end
            READ_WORD: begin
                if (read_payload && !payload_failed) begin
                    payload_timer <= 0; payload_sent <= 0;
                    // The timeout also covers failure to accept the request.
                    state <= PAYLOAD_WAIT;
                end else begin
                    if (read_payload) read_data <= 32'hffffffff;
                    else if (read_rom) read_data <= rom[{rom_page,word_addr[11:2]}];
                    else read_data <= journal_data;
                    state <= STORE_WORD;
                end
            end
            PAYLOAD_WAIT: begin
                if (payload_request && payload_ready) payload_sent <= 1;
                if (payload_response_valid && payload_sent) begin
                    read_data <= payload_response_error ? 32'hffffffff : payload_response_data;
                    if (payload_response_error) begin rx_fault <= 1; payload_failed <= 1; end
                    state <= STORE_WORD;
                end else if (payload_timer == PAYLOAD_TIMEOUT_CYCLES-1) begin
                    // Complete bounded reads with erased data after a transport
                    // failure, latch diagnostics, and cancel the engine. The DXE
                    // envelope/hash validation rejects this data; never hang PCIe.
                    read_data <= 32'hffffffff; rx_fault <= 1; payload_failed <= 1;
                    state <= STORE_WORD;
                end else payload_timer <= payload_timer+1;
            end
            STORE_WORD: begin
                reply[3+index] <= swap_bytes(read_data);
                if (index+1 == chunk_words(current)) state <= TX;
                else begin index <= index+1; state <= READ_WORD; end
            end
            TX: if (tx_ready) begin
                if (tx_last) begin
                    if (current.ur || current.words == chunk_words(current)) state <= WAIT_RETIRE;
                    else begin current <= advance_token(current); state <= HEADER; end
                end else tx_index <= tx_index+2;
            end
            WAIT_RETIRE: if (token_ready) begin state <= RX; received <= 0; bad <= 0; end
            WRITE_WORD: begin
                if (index+1 == length_dw) begin
                    state <= RX; received <= 0;
                end else index <= index+1;
            end
            default: begin rx_fault <= 1; state <= RX; received <= 0; bad <= 0; end
        endcase
    end
endmodule

