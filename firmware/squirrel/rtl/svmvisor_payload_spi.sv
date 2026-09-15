`timescale 1ns/1ps
// Read-only configuration flash transport. IS25LP256D Rev.A section 8.3:
// opcode 13h always takes four address bytes, independent of EXTADD.
// No host-selected command, write enable, erase, program or mode change exists.
module svmvisor_payload_spi (
    input logic clk, rst, cancel, configuration_done,
    input logic request,
    input logic [19:0] address,
    output logic ready, response_valid, response_error,
    output logic [31:0] response_data,
    output logic spi_clk = 0, spi_cs_n = 1, spi_mosi = 0,
    input logic spi_miso
);
    typedef enum logic [2:0] {PRIME, IDLE, SHIFT, FINISH} state_t;
    state_t state = PRIME;
    logic [3:0] prime_edges = 0;
    logic half_cycle = 0;
    logic [6:0] bit_count = 0;
    logic [39:0] command = 0;
    logic [31:0] incoming = 0;
    (* ASYNC_REG="TRUE" *) logic [1:0] eos_sync = 0;
    always @(posedge clk) begin
        if (rst) eos_sync <= 0;
        else eos_sync <= {eos_sync[0],configuration_done};
    end
    wire startup_reset = rst || !eos_sync[1];
    // Capture exactly once per serial bit, 8 ns after the rising SCK drive
    // edge. The prior falling drive edge is 40 ns earlier at 15.625 MHz.
    // half_cycle==0 is true only on the first falling user edge after a
    // serial transition. The next falling user edge (half_cycle==1) is idle.
    always @(negedge clk) begin
        if (startup_reset || cancel || state == IDLE || state == PRIME) incoming <= 0;
        else if (state == SHIFT && spi_clk && !half_cycle && bit_count >= 40)
            incoming <= {incoming[30:0],spi_miso};
    end
    assign ready = state == IDLE && !startup_reset && !cancel;
    // PRIME clocks with CE# high allow STARTUPE2's initial three swallowed
    // USRCCLKO cycles (UG470) before any command is sent. Repeat after reset.
    always @(posedge clk) begin
        if (startup_reset || cancel) begin
            state <= PRIME; prime_edges <= 0; half_cycle <= 0; spi_cs_n <= 1; spi_clk <= 0;
            spi_mosi <= 0; response_valid <= 0; response_error <= 0;
            response_data <= 32'hffffffff; bit_count <= 0; command <= 0;
        end else begin
            response_valid <= 0; response_error <= 0;
            case (state)
                PRIME: begin
                    half_cycle <= !half_cycle;
                    if (half_cycle) begin
                        spi_clk <= !spi_clk;
                        if (prime_edges == 7) begin spi_clk <= 0; state <= IDLE; end
                        else prime_edges <= prime_edges+1;
                    end
                end
                IDLE: if (request) begin
                    if (address[1:0] != 0) begin
                        response_valid <= 1; response_error <= 1; response_data <= 32'hffffffff;
                    end else begin
                        // Concatenation restricts every address to [4 MiB,5 MiB).
                        command <= {8'h13,12'h004,address};
                        spi_cs_n <= 0; spi_mosi <= 0; spi_clk <= 0;
                        bit_count <= 0; half_cycle <= 0; state <= SHIFT;
                    end
                end
                SHIFT: begin
                    half_cycle <= !half_cycle;
                    if (half_cycle) begin
                        spi_clk <= !spi_clk;
                        if (spi_clk) begin
                            if (bit_count == 71) begin spi_clk <= 0; state <= FINISH; end
                            else begin
                                bit_count <= bit_count+1;
                                command <= {command[38:0],1'b0};
                                spi_mosi <= command[38];
                            end
                        end
                    end
                end
                FINISH: begin
                    // Keep CE# asserted for two user periods after the final
                    // falling drive edge, covering routed CCLK+STARTUP delay.
                    if (!half_cycle) half_cycle <= 1;
                    else begin
                        half_cycle <= 0; spi_cs_n <= 1; spi_mosi <= 0;
                        response_data <= {incoming[7:0],incoming[15:8],incoming[23:16],incoming[31:24]};
                        response_valid <= 1; state <= IDLE;
                    end
                end
                default: begin
                    spi_cs_n <= 1; spi_clk <= 0; spi_mosi <= 0;
                    response_valid <= 1; response_error <= 1;
                    response_data <= 32'hffffffff; state <= PRIME; prime_edges <= 0;
                end
            endcase
        end
    end
endmodule
