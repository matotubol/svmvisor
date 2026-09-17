// Sample the active-low board switch once, before releasing PCIe reset.
// Only platform PERST# (or FPGA reconfiguration) can start a new sample.
module svmvisor_bypass(
    input wire clk, perst_n, switch_n,
    output reg bypass = 1'b1,
    output reg core_release = 1'b0
);
    (* ASYNC_REG = "TRUE" *) reg [1:0] switch_sync = 2'b00;
    (* ASYNC_REG = "TRUE" *) reg [1:0] reset_sync = 2'b00;
    always @(posedge clk or negedge perst_n) begin
        if (!perst_n) reset_sync <= 0;
        else reset_sync <= {reset_sync[0],1'b1};
    end
    reg [5:0] startup = 0;
    always @(posedge clk or negedge reset_sync[1]) begin
        if (!reset_sync[1]) begin
            switch_sync <= 0; startup <= 0; bypass <= 1; core_release <= 0;
        end else begin
            switch_sync <= {switch_sync[0],switch_n};
            if (startup != 63) begin
                startup <= startup+1;
                if (startup == 62) begin
                    bypass <= !switch_sync[1];
                    core_release <= switch_sync[1];
                end
            end
        end
    end
endmodule
