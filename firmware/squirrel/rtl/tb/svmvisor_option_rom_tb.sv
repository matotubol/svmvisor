`timescale 1ns / 1ps

module svmvisor_option_rom_tb;
    reg         rst = 1'b1;
    reg         clk = 1'b0;
    reg [31:0]  wr_addr = 32'b0;
    reg [3:0]   wr_be = 4'b0;
    reg [31:0]  wr_data = 32'b0;
    reg         wr_valid = 1'b0;
    reg [87:0]  rd_req_ctx = 88'b0;
    reg [31:0]  rd_req_addr = 32'b0;
    reg         rd_req_valid = 1'b0;
    wire [87:0] rd_rsp_ctx;
    wire [31:0] rd_rsp_data;
    wire        rd_rsp_valid;

    svmvisor_option_rom #(
        .INIT_FILE("")
    ) dut (
        .rst(rst),
        .clk(clk),
        .wr_addr(wr_addr),
        .wr_be(wr_be),
        .wr_data(wr_data),
        .wr_valid(wr_valid),
        .rd_req_ctx(rd_req_ctx),
        .rd_req_addr(rd_req_addr),
        .rd_req_valid(rd_req_valid),
        .rd_rsp_ctx(rd_rsp_ctx),
        .rd_rsp_data(rd_rsp_data),
        .rd_rsp_valid(rd_rsp_valid)
    );

    always #5 clk = ~clk;

    task expect_response;
        input [87:0] expected_ctx;
        input [31:0] expected_data;
        begin
            if (!rd_rsp_valid)
                $fatal(1, "expected a valid ROM response");
            if (rd_rsp_ctx !== expected_ctx)
                $fatal(1, "context mismatch: got %h, expected %h", rd_rsp_ctx, expected_ctx);
            if (rd_rsp_data !== expected_data)
                $fatal(1, "data mismatch: got %h, expected %h", rd_rsp_data, expected_data);
        end
    endtask

    initial begin
        #1;
        dut.rom[0] = 32'h0007aa55;
        dut.rom[1] = 32'h52494350;

        repeat (2) @(negedge clk);
        rst = 1'b0;

        // Two back-to-back reads prove context and data remain aligned through
        // the two-clock BRAM-compatible pipeline.
        rd_req_ctx = 88'h0000000000000000000001;
        rd_req_addr = 32'h00000000;
        rd_req_valid = 1'b1;
        @(negedge clk);

        rd_req_ctx = 88'h0000000000000000000002;
        rd_req_addr = 32'h00000004;
        @(negedge clk);

        rd_req_valid = 1'b0;
        expect_response(88'h0000000000000000000001, 32'h0007aa55);
        @(negedge clk);
        expect_response(88'h0000000000000000000002, 32'h52494350);

        // A read from the final DWORD of the aperture must return erased ROM,
        // not wrap to DWORD zero.
        rd_req_ctx = 88'h0000000000000000000003;
        rd_req_addr = 32'h00000ffc;
        rd_req_valid = 1'b1;
        @(negedge clk);
        rd_req_valid = 1'b0;
        @(negedge clk);
        expect_response(88'h0000000000000000000003, 32'hffffffff);

        // Writes are ignored.
        wr_addr = 32'h0;
        wr_be = 4'hf;
        wr_data = 32'hdeadbeef;
        wr_valid = 1'b1;
        @(negedge clk);
        wr_valid = 1'b0;
        rd_req_ctx = 88'h0000000000000000000004;
        rd_req_addr = 32'h0;
        rd_req_valid = 1'b1;
        @(negedge clk);
        rd_req_valid = 1'b0;
        @(negedge clk);
        expect_response(88'h0000000000000000000004, 32'h0007aa55);

        $display("svmvisor_option_rom_tb: PASS");
        $finish;
    end

endmodule
