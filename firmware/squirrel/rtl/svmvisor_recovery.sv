// Non-enumerating recovery: no clock, PCIe core, transceiver, or command path.
// Dedicated configuration JTAG remains available independently of fabric logic.
module svmvisor_recovery (
    output wire user_ld1,
    output wire user_ld2,
    output wire ft2232_rst_n,
    output wire ft601_rst_n,
    output wire ft601_wr_n,
    output wire ft601_rd_n,
    output wire ft601_oe_n,
    output wire ft601_siwu_n,
    output wire pcie_wake_n
);
    // Opposite static LED levels make recovery visible with either LED polarity.
    assign user_ld1 = 1'b1;
    assign user_ld2 = 1'b0;
    assign ft2232_rst_n = 1'b1;
    assign ft601_rst_n = 1'b0;
    assign ft601_wr_n = 1'b1;
    assign ft601_rd_n = 1'b1;
    assign ft601_oe_n = 1'b1;
    assign ft601_siwu_n = 1'b1;
    assign pcie_wake_n = 1'b1;
endmodule
