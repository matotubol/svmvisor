`timescale 1ns / 1ps

module svmvisor_tlps128_bar_rdengine_tb;
    reg rst = 1'b1;
    reg clk = 1'b0;
    reg [127:0] request_tdata = 128'd0;
    reg [3:0] request_tkeepdw = 4'd0;
    reg request_tvalid = 1'b0;
    reg request_tlast = 1'b0;
    reg [8:0] request_tuser = 9'd0;
    reg request_valid = 1'b0;

    IfAXIS128 tlps_in();
    IfAXIS128 tlps_out();

    wire [87:0] rd_req_ctx;
    wire [6:0] rd_req_bar;
    wire [31:0] rd_req_addr;
    wire rd_req_valid;
    wire [87:0] rd_rsp_ctx;
    wire [31:0] rd_rsp_data;
    wire rd_rsp_valid;

    assign tlps_in.tdata = request_tdata;
    assign tlps_in.tkeepdw = request_tkeepdw;
    assign tlps_in.tvalid = request_tvalid;
    assign tlps_in.tlast = request_tlast;
    assign tlps_in.tuser = request_tuser;
    assign tlps_in.has_data = request_tvalid;
    assign tlps_out.tready = 1'b1;

    svmvisor_tlps128_bar_rdengine dut (
        .rst(rst),
        .clk(clk),
        .pcie_id(16'h1234),
        .tlps_in(tlps_in),
        .tlps_in_valid(request_valid),
        .tlps_out(tlps_out),
        .rd_req_ctx(rd_req_ctx),
        .rd_req_bar(rd_req_bar),
        .rd_req_addr(rd_req_addr),
        .rd_req_valid(rd_req_valid),
        .rd_rsp_ctx(rd_rsp_ctx),
        .rd_rsp_data(rd_rsp_data),
        .rd_rsp_valid(rd_rsp_valid)
    );

    svmvisor_option_rom #(
        .INIT_FILE("")
    ) option_rom (
        .rst(rst),
        .clk(clk),
        .wr_addr(32'd0),
        .wr_be(4'd0),
        .wr_data(32'd0),
        .wr_valid(1'b0),
        .rd_req_ctx(rd_req_ctx),
        .rd_req_addr(rd_req_addr),
        .rd_req_valid(rd_req_valid && rd_req_bar[6]),
        .rd_rsp_ctx(rd_rsp_ctx),
        .rd_rsp_data(rd_rsp_data),
        .rd_rsp_valid(rd_rsp_valid)
    );

    always #5 clk = ~clk;
    always @(negedge clk) begin
        if (!rst && (dut.rd1_out_valid !== dut.rd1_out_metadata_valid))
            $fatal(1, "request and byte-enable metadata FIFOs lost alignment");
    end

    function automatic [127:0] make_read_header;
        input [9:0] length_dw;
        input [3:0] first_be;
        input [3:0] last_be;
        input [31:0] address;
        input [15:0] requester_id;
        input [7:0] tag;
        reg [127:0] header;
        begin
            header = 128'd0;
            header[31:25] = 7'b0000000;
            header[9:0] = length_dw;
            header[63:48] = requester_id;
            header[47:40] = tag;
            header[39:36] = last_be;
            header[35:32] = first_be;
            header[95:66] = address[31:2];
            make_read_header = header;
        end
    endfunction

    task automatic send_read_request;
        input [9:0] length_dw;
        input [3:0] first_be;
        input [3:0] last_be;
        input [31:0] address;
        input [15:0] requester_id;
        input [7:0] tag;
        begin
            @(negedge clk);
            request_tdata = make_read_header(
                length_dw,
                first_be,
                last_be,
                address,
                requester_id,
                tag
            );
            request_tkeepdw = 4'b0111;
            request_tvalid = 1'b1;
            request_tlast = 1'b1;
            request_tuser = 9'd0;
            request_tuser[0] = 1'b1;
            request_tuser[1] = 1'b1;
            request_tuser[8] = 1'b1;
            request_valid = 1'b1;
            @(negedge clk);
            request_tvalid = 1'b0;
            request_tlast = 1'b0;
            request_tuser = 9'd0;
            request_valid = 1'b0;
        end
    endtask

    task automatic expect_completion_beat;
        input [9:0] expected_length_dw;
        input [11:0] expected_byte_count;
        input [6:0] expected_lower_address;
        input [15:0] expected_requester_id;
        input [7:0] expected_tag;
        input expected_first;
        input expected_last;
        input [3:0] expected_keep;
        input [31:0] expected_payload;
        integer clocks_waited;
        begin
            clocks_waited = 0;
            while (!tlps_out.tvalid && clocks_waited < 200) begin
                @(negedge clk);
                clocks_waited = clocks_waited + 1;
            end
            if (!tlps_out.tvalid)
                $fatal(1, "timed out waiting for completion");
            if (tlps_out.tuser[0] !== expected_first)
                $fatal(1, "first mismatch: got %b, expected %b", tlps_out.tuser[0], expected_first);
            if (tlps_out.tlast !== expected_last)
                $fatal(1, "last mismatch: got %b, expected %b", tlps_out.tlast, expected_last);
            if (tlps_out.tkeepdw !== expected_keep)
                $fatal(1, "keep mismatch: got %b, expected %b", tlps_out.tkeepdw, expected_keep);

            if (expected_first) begin
                if (tlps_out.tdata[9:0] !== expected_length_dw)
                    $fatal(1, "length mismatch: got %0d, expected %0d", tlps_out.tdata[9:0], expected_length_dw);
                if (tlps_out.tdata[43:32] !== expected_byte_count)
                    $fatal(1, "byte count mismatch: got %0d, expected %0d", tlps_out.tdata[43:32], expected_byte_count);
                if (tlps_out.tdata[95:80] !== expected_requester_id)
                    $fatal(1, "requester mismatch: got %h, expected %h", tlps_out.tdata[95:80], expected_requester_id);
                if (tlps_out.tdata[79:72] !== expected_tag)
                    $fatal(1, "tag mismatch: got %h, expected %h", tlps_out.tdata[79:72], expected_tag);
                if (tlps_out.tdata[70:64] !== expected_lower_address)
                    $fatal(1, "lower address mismatch: got %h, expected %h", tlps_out.tdata[70:64], expected_lower_address);
                if (tlps_out.tdata[127:96] !== expected_payload)
                    $fatal(1, "first payload mismatch: got %h, expected %h", tlps_out.tdata[127:96], expected_payload);
            end else if (tlps_out.tdata[31:0] !== expected_payload) begin
                $fatal(1, "continuation payload mismatch: got %h, expected %h", tlps_out.tdata[31:0], expected_payload);
            end

            @(negedge clk);
        end
    endtask

    initial begin
        #1;
        option_rom.rom[0] = 32'h0007aa55;
        option_rom.rom[1] = 32'h44332211;
        option_rom.rom[15] = 32'hd4c3b2a1;
        option_rom.rom[16] = 32'h88776655;

        repeat (3) @(negedge clk);
        rst = 1'b0;

        // A firmware byte read at offset +1 must report exactly one byte and
        // carry that offset in Completion Lower Address.
        send_read_request(10'd1, 4'b0010, 4'b0000, 32'h00000000, 16'ha1b2, 8'h11);
        expect_completion_beat(10'd1, 12'd1, 7'h01, 16'ha1b2, 8'h11, 1'b1, 1'b1, 4'b1111, 32'h55aa0700);

        // A normal full-DWORD read retains the aligned lower address.
        send_read_request(10'd1, 4'b1111, 4'b0000, 32'h00000004, 16'hc3d4, 8'h22);
        expect_completion_beat(10'd1, 12'd4, 7'h04, 16'hc3d4, 8'h22, 1'b1, 1'b1, 4'b1111, 32'h11223344);

        // For legal non-contiguous one-DWORD masks, Byte Count describes the
        // requested span, including holes, rather than the number of set bits.
        send_read_request(10'd1, 4'b1001, 4'b0000, 32'h00000000, 16'h2468, 8'h23);
        expect_completion_beat(10'd1, 12'd4, 7'h00, 16'h2468, 8'h23, 1'b1, 1'b1, 4'b1111, 32'h55aa0700);
        send_read_request(10'd1, 4'b0101, 4'b0000, 32'h00000000, 16'h2468, 8'h24);
        expect_completion_beat(10'd1, 12'd3, 7'h00, 16'h2468, 8'h24, 1'b1, 1'b1, 4'b1111, 32'h55aa0700);

        // First DW BE 0000 is the zero-length-read encoding. PCIe still
        // requires one data DWORD and a Completion Byte Count of one.
        send_read_request(10'd1, 4'b0000, 4'b0000, 32'h00000000, 16'h2468, 8'h25);
        expect_completion_beat(10'd1, 12'd1, 7'h00, 16'h2468, 8'h25, 1'b1, 1'b1, 4'b1111, 32'h55aa0700);

        // First/Last DW byte enables contribute to one Byte Count while both
        // full payload DWORDs are returned.
        send_read_request(10'd2, 4'b1110, 4'b0011, 32'h00000000, 16'he5f6, 8'h33);
        expect_completion_beat(10'd2, 12'd5, 7'h01, 16'he5f6, 8'h33, 1'b1, 1'b0, 4'b1111, 32'h55aa0700);
        expect_completion_beat(10'd0, 12'd0, 7'd0, 16'd0, 8'd0, 1'b0, 1'b1, 4'b0001, 32'h11223344);

        // A request crossing the default 64-byte RCB becomes two completions. The
        // second Completion Byte Count is the two bytes that remain, and its
        // lower address starts at the next aligned boundary.
        send_read_request(10'd2, 4'b1110, 4'b0011, 32'h0000003c, 16'h1357, 8'h44);
        expect_completion_beat(10'd1, 12'd5, 7'h3d, 16'h1357, 8'h44, 1'b1, 1'b1, 4'b1111, 32'ha1b2c3d4);
        expect_completion_beat(10'd1, 12'd2, 7'h40, 16'h1357, 8'h44, 1'b1, 1'b1, 4'b1111, 32'h55667788);

        // Consecutive request clocks prove the parallel byte-enable metadata
        // queue stays aligned with the pinned request FIFO.
        @(negedge clk);
        request_tdata = make_read_header(10'd1, 4'b1111, 4'b0000, 32'h00000000, 16'h9876, 8'h51);
        request_tkeepdw = 4'b0111;
        request_tvalid = 1'b1;
        request_tlast = 1'b1;
        request_tuser = 9'd0;
        request_tuser[0] = 1'b1;
        request_tuser[1] = 1'b1;
        request_tuser[8] = 1'b1;
        request_valid = 1'b1;
        @(negedge clk);
        request_tdata = make_read_header(10'd1, 4'b1111, 4'b0000, 32'h00000004, 16'h9876, 8'h52);
        @(negedge clk);
        request_tvalid = 1'b0;
        request_tlast = 1'b0;
        request_tuser = 9'd0;
        request_valid = 1'b0;
        expect_completion_beat(10'd1, 12'd4, 7'h00, 16'h9876, 8'h51, 1'b1, 1'b1, 4'b1111, 32'h55aa0700);
        expect_completion_beat(10'd1, 12'd4, 7'h04, 16'h9876, 8'h52, 1'b1, 1'b1, 4'b1111, 32'h11223344);

        $display("svmvisor_tlps128_bar_rdengine_tb: PASS");
        $finish;
    end
endmodule
