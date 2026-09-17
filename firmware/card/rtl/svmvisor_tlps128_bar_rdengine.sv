`timescale 1ns / 1ps
`include "pcileech_header.svh"

// BAR Memory Read completer for the PCILeech AXIS128 stream.
//
// This is a drop-in replacement for the upstream read engine. It preserves
// the upstream request splitting and BAR-leaf interface, but also carries the
// Memory Read First/Last DW Byte Enables so Completion Byte Count and Lower
// Address are correct for byte and other partial-DWORD reads.
module svmvisor_tlps128_bar_rdengine(
    input                   rst,
    input                   clk,
    input [15:0]            pcie_id,
    IfAXIS128.sink_lite     tlps_in,
    input                   tlps_in_valid,
    IfAXIS128.source        tlps_out,
    output [87:0]           rd_req_ctx,
    output [6:0]            rd_req_bar,
    output [31:0]           rd_req_addr,
    output                  rd_req_valid,
    input [87:0]            rd_rsp_ctx,
    input [31:0]            rd_rsp_data,
    input                   rd_rsp_valid
);

    function automatic [1:0] first_enabled_offset(input [3:0] byte_enable);
        if (byte_enable[0])
            first_enabled_offset = 2'd0;
        else if (byte_enable[1])
            first_enabled_offset = 2'd1;
        else if (byte_enable[2])
            first_enabled_offset = 2'd2;
        else if (byte_enable[3])
            first_enabled_offset = 2'd3;
        else
            first_enabled_offset = 2'd0;
    endfunction

    function automatic [1:0] last_enabled_offset(input [3:0] byte_enable);
        if (byte_enable[3])
            last_enabled_offset = 2'd3;
        else if (byte_enable[2])
            last_enabled_offset = 2'd2;
        else if (byte_enable[1])
            last_enabled_offset = 2'd1;
        else
            last_enabled_offset = 2'd0;
    endfunction

    // ---------------------------------------------------------------------
    // 1: Queue incoming Memory Read requests.
    // ---------------------------------------------------------------------
    wire [10:0] rd1_in_dwlen =
        (tlps_in.tdata[9:0] == 0) ? 11'd1024 : {1'b0, tlps_in.tdata[9:0]};
    wire [6:0] rd1_in_bar = tlps_in.tuser[8:2];
    wire [15:0] rd1_in_reqid = tlps_in.tdata[63:48];
    wire [7:0] rd1_in_tag = tlps_in.tdata[47:40];
    wire [3:0] rd1_in_first_be = tlps_in.tdata[35:32];
    wire [3:0] rd1_in_last_be = tlps_in.tdata[39:36];
    wire [1:0] rd1_in_first_offset =
        first_enabled_offset(rd1_in_first_be);
    wire [1:0] rd1_in_last_offset =
        (rd1_in_last_be == 0)
            ? 2'd3
            : last_enabled_offset(rd1_in_last_be);
    wire [1:0] rd1_in_single_last_offset =
        last_enabled_offset(rd1_in_first_be);
    wire rd1_in_zero_length =
        (rd1_in_dwlen == 1) && (rd1_in_first_be == 0);

    // PCIe Byte Count is the span from the first enabled byte through the
    // last enabled byte, including any holes in a legal single-DWORD mask.
    // A First DW BE of zero encodes a zero-length read whose CplD has BC=1.
    wire [12:0] rd1_in_byte_count =
        rd1_in_zero_length
            ? 13'd1
            : (rd1_in_dwlen == 1)
                ? {11'b0, rd1_in_single_last_offset}
                    - {11'b0, rd1_in_first_offset}
                    + 13'd1
                : (({2'b0, rd1_in_dwlen} - 13'd1) << 2)
                    + {11'b0, rd1_in_last_offset}
                    + 13'd1
                    - {11'b0, rd1_in_first_offset};
    wire [31:0] rd1_in_addr = {
        (tlps_in.tdata[31:29] == 3'b000)
            ? tlps_in.tdata[95:66]
            : tlps_in.tdata[127:98],
        rd1_in_first_offset
    };

    wire [73:0] rd1_in_data = {
        rd1_in_dwlen,
        rd1_in_bar,
        rd1_in_tag,
        rd1_in_reqid,
        rd1_in_addr
    };
    wire [15:0] rd1_in_metadata = {
        1'b0,
        rd1_in_first_offset,
        rd1_in_byte_count
    };

    wire rd1_out_rden;
    wire [73:0] rd1_out_data;
    wire rd1_out_valid;
    wire rd1_fifo_full;
    wire rd1_write = tlps_in_valid && !rd1_fifo_full;

    fifo_74_74_clk1_bar_rd1 i_fifo_74_74_clk1_bar_rd1(
        .srst       ( rst             ),
        .clk        ( clk             ),
        .wr_en      ( rd1_write       ),
        .din        ( rd1_in_data     ),
        .full       ( rd1_fifo_full   ),
        .rd_en      ( rd1_out_rden    ),
        .dout       ( rd1_out_data    ),
        .empty      (                 ),
        .valid      ( rd1_out_valid   )
    );

    // The pinned request FIFO is 74 bits wide. A parallel, identically-clocked
    // queue carries the byte-count metadata that the upstream engine discarded.
    (* ram_style = "distributed" *) reg [15:0] rd1_metadata_fifo [0:511];
    reg [8:0] rd1_metadata_wr_ptr;
    reg [8:0] rd1_metadata_rd_ptr;
    reg [9:0] rd1_metadata_count;
    reg [15:0] rd1_out_metadata;
    reg rd1_out_metadata_valid;
    wire rd1_metadata_read = rd1_out_rden && (rd1_metadata_count != 0);

    always @(posedge clk) begin
        if (rst) begin
            rd1_metadata_wr_ptr <= 9'd0;
            rd1_metadata_rd_ptr <= 9'd0;
            rd1_metadata_count <= 10'd0;
            rd1_out_metadata <= 16'd0;
            rd1_out_metadata_valid <= 1'b0;
        end else begin
            rd1_out_metadata_valid <= rd1_metadata_read;

            if (rd1_write) begin
                rd1_metadata_fifo[rd1_metadata_wr_ptr] <= rd1_in_metadata;
                rd1_metadata_wr_ptr <= rd1_metadata_wr_ptr + 1'b1;
            end
            if (rd1_metadata_read) begin
                rd1_out_metadata <= rd1_metadata_fifo[rd1_metadata_rd_ptr];
                rd1_metadata_rd_ptr <= rd1_metadata_rd_ptr + 1'b1;
            end

            case ({rd1_write, rd1_metadata_read})
                2'b10: rd1_metadata_count <= rd1_metadata_count + 1'b1;
                2'b01: rd1_metadata_count <= rd1_metadata_count - 1'b1;
                default: rd1_metadata_count <= rd1_metadata_count;
            endcase
        end
    end

    wire rd1_pair_valid = rd1_out_valid && rd1_out_metadata_valid;
    wire [1:0] rd1_out_first_offset = rd1_out_metadata[14:13];
    wire [12:0] rd1_out_byte_count = rd1_out_metadata[12:0];

    // ---------------------------------------------------------------------
    // 2: Split requests at the default 64-byte Read Completion Boundary.
    // PG054 requires user logic that does not read the negotiated RCB value
    // to use 64 bytes.
    // ---------------------------------------------------------------------
    wire [10:0] rd1_out_dwlen = rd1_out_data[73:63];
    wire [3:0] rd1_out_dwlen4 = rd1_out_data[66:63];
    wire [3:0] rd1_out_addr4 = rd1_out_data[5:2];
    wire [4:0] rd1_out_boundary_sum =
        {1'b0, rd1_out_addr4} + {1'b0, rd1_out_dwlen4};

    wire [3:0] rd2_pkt1_dwlen_pre =
        ((rd1_out_boundary_sum > 5'h10)
            || ((rd1_out_addr4 != 0) && (rd1_out_dwlen4 == 0)))
        ? (5'h10 - rd1_out_addr4)
        : rd1_out_dwlen4;
    wire [4:0] rd2_pkt1_dwlen =
        (rd2_pkt1_dwlen_pre == 0) ? 5'h10 : rd2_pkt1_dwlen_pre;
    wire [10:0] rd2_pkt1_dwlen_next = rd1_out_dwlen - rd2_pkt1_dwlen;
    wire rd2_pkt1_large =
        (rd1_out_dwlen > 16) || (rd1_out_dwlen != rd2_pkt1_dwlen);
    wire rd2_pkt1_tiny = (rd1_out_dwlen == 1);
    wire [12:0] rd2_pkt1_returned_bytes =
        ({8'b0, rd2_pkt1_dwlen} << 2)
        - {11'b0, rd1_out_first_offset};
    wire [12:0] rd2_pkt1_byte_count_next =
        rd1_out_byte_count - rd2_pkt1_returned_bytes;

    wire [85:0] rd2_pkt1;
    assign rd2_pkt1[85:74] = rd1_out_byte_count[11:0];
    assign rd2_pkt1[73:63] = rd2_pkt1_dwlen;
    assign rd2_pkt1[62:0] = rd1_out_data[62:0];

    reg [10:0] rd2_total_dwlen;
    wire [10:0] rd2_total_dwlen_next = rd2_total_dwlen - 11'h10;
    reg [12:0] rd2_total_byte_count;
    reg [85:0] rd2_pkt2;
    wire [10:0] rd2_pkt2_dwlen = rd2_pkt2[73:63];
    wire rd2_pkt2_large = (rd2_total_dwlen > 11'h10);
    wire [12:0] rd2_pkt2_returned_bytes = {rd2_pkt2_dwlen, 2'b00};
    wire [12:0] rd2_total_byte_count_next =
        rd2_total_byte_count - rd2_pkt2_returned_bytes;
    wire rd2_out_rden;

    localparam SVM_S2_REQDATA = 1'b0;
    localparam SVM_S2_PROCESSING = 1'b1;
    reg state2 = SVM_S2_REQDATA;

    always @(posedge clk) begin
        if (rst) begin
            state2 <= SVM_S2_REQDATA;
            rd2_total_dwlen <= 11'd0;
            rd2_total_byte_count <= 13'd0;
            rd2_pkt2 <= 86'd0;
        end else begin
            case (state2)
                SVM_S2_REQDATA: begin
                    if (rd1_pair_valid && rd2_pkt1_large) begin
                        rd2_total_dwlen <= rd2_pkt1_dwlen_next;
                        rd2_total_byte_count <= rd2_pkt1_byte_count_next;
                        rd2_pkt2[85:74] <= rd2_pkt1_byte_count_next[11:0];
                        rd2_pkt2[73:63] <=
                            (rd2_pkt1_dwlen_next > 11'h10)
                                ? 11'h10
                                : rd2_pkt1_dwlen_next;
                        rd2_pkt2[62:12] <= rd1_out_data[62:12];
                        rd2_pkt2[11:0] <=
                            {rd1_out_data[11:2], 2'b00}
                            + {rd2_pkt1_dwlen, 2'b00};
                        state2 <= SVM_S2_PROCESSING;
                    end
                end
                SVM_S2_PROCESSING: begin
                    if (rd2_out_rden) begin
                        rd2_total_dwlen <= rd2_total_dwlen_next;
                        rd2_total_byte_count <= rd2_total_byte_count_next;
                        rd2_pkt2[85:74] <= rd2_total_byte_count_next[11:0];
                        rd2_pkt2[73:63] <=
                            (rd2_total_dwlen_next > 11'h10)
                                ? 11'h10
                                : rd2_total_dwlen_next;
                        rd2_pkt2[62:12] <= rd2_pkt2[62:12];
                        rd2_pkt2[11:0] <=
                            {rd2_pkt2[11:2], 2'b00}
                            + {rd2_pkt2_dwlen, 2'b00};
                        if (!rd2_pkt2_large)
                            state2 <= SVM_S2_REQDATA;
                    end
                end
            endcase
        end
    end

    assign rd1_out_rden = rd2_out_rden && (
        ((state2 == SVM_S2_REQDATA)
            && (!rd1_pair_valid || rd2_pkt1_tiny))
        || ((state2 == SVM_S2_PROCESSING) && !rd2_pkt2_large)
    );

    wire [85:0] rd2_in_data =
        (state2 == SVM_S2_REQDATA) ? rd2_pkt1 : rd2_pkt2;
    wire rd2_in_valid = rd1_pair_valid
        || ((state2 == SVM_S2_PROCESSING) && rd2_out_rden);

    reg [85:0] rd2_out_data;
    reg rd2_out_valid;
    always @(posedge clk) begin
        rd2_out_data <= rd2_in_valid ? rd2_in_data : rd2_out_data;
        rd2_out_valid <= rd2_in_valid && !rst;
    end

    // ---------------------------------------------------------------------
    // 3: Issue one BAR-leaf DWORD read per clock.
    // ---------------------------------------------------------------------
    wire [4:0] rd2_out_dwlen = rd2_out_data[67:63];
    wire rd2_out_last = (rd2_out_dwlen == 1);
    wire [9:0] rd2_out_dwaddr = rd2_out_data[11:2];
    wire rd3_enable;

    reg rd3_process_valid;
    reg rd3_process_first;
    reg rd3_process_last;
    reg [4:0] rd3_process_dwlen;
    reg [9:0] rd3_process_dwaddr;
    reg [85:0] rd3_process_data;
    wire rd3_process_next_last = (rd3_process_dwlen == 2);
    wire rd3_process_nextnext_last = (rd3_process_dwlen <= 3);

    assign rd_req_ctx = {
        rd3_process_first,
        rd3_process_last,
        rd3_process_data
    };
    assign rd_req_bar = rd3_process_data[62:56];
    assign rd_req_addr = {
        rd3_process_data[31:12],
        rd3_process_dwaddr,
        2'b00
    };
    assign rd_req_valid = rd3_process_valid;

    localparam SVM_S3_REQDATA = 1'b0;
    localparam SVM_S3_PROCESSING = 1'b1;
    reg state3 = SVM_S3_REQDATA;

    always @(posedge clk) begin
        if (rst) begin
            rd3_process_valid <= 1'b0;
            state3 <= SVM_S3_REQDATA;
        end else begin
            case (state3)
                SVM_S3_REQDATA: begin
                    if (rd2_out_valid) begin
                        rd3_process_valid <= 1'b1;
                        rd3_process_first <= 1'b1;
                        rd3_process_last <= rd2_out_last;
                        rd3_process_dwlen <= rd2_out_dwlen;
                        rd3_process_dwaddr <= rd2_out_dwaddr;
                        rd3_process_data <= rd2_out_data;
                        if (!rd2_out_last)
                            state3 <= SVM_S3_PROCESSING;
                    end else begin
                        rd3_process_valid <= 1'b0;
                    end
                end
                SVM_S3_PROCESSING: begin
                    rd3_process_first <= 1'b0;
                    rd3_process_last <= rd3_process_next_last;
                    rd3_process_dwlen <= rd3_process_dwlen - 1'b1;
                    rd3_process_dwaddr <= rd3_process_dwaddr + 1'b1;
                    if (rd3_process_next_last)
                        state3 <= SVM_S3_REQDATA;
                end
            endcase
        end
    end

    assign rd2_out_rden = rd3_enable && (
        ((state3 == SVM_S3_REQDATA)
            && (!rd2_out_valid || rd2_out_last))
        || ((state3 == SVM_S3_PROCESSING)
            && rd3_process_nextnext_last)
    );

    // ---------------------------------------------------------------------
    // 4: Assemble Completion-with-Data TLPs and queue them for the TX mux.
    // ---------------------------------------------------------------------
    wire rd_rsp_first = rd_rsp_ctx[87];
    wire rd_rsp_last = rd_rsp_ctx[86];
    wire [9:0] rd_rsp_dwlen = rd_rsp_ctx[72:63];
    wire [11:0] rd_rsp_byte_count = rd_rsp_ctx[85:74];
    wire [15:0] rd_rsp_reqid = rd_rsp_ctx[47:32];
    wire [7:0] rd_rsp_tag = rd_rsp_ctx[55:48];
    wire [6:0] rd_rsp_lowaddr = rd_rsp_ctx[6:0];
    wire [31:0] rd_rsp_data_wire_order = {
        rd_rsp_data[7:0],
        rd_rsp_data[15:8],
        rd_rsp_data[23:16],
        rd_rsp_data[31:24]
    };

    reg [127:0] completion_data;
    reg [3:0] completion_keep = 4'b0;
    reg completion_last;
    reg completion_first = 1'b1;
    wire completion_valid = completion_last || completion_keep[3];

    always @(posedge clk) begin
        if (rst) begin
            completion_keep <= 4'b0;
            completion_last <= 1'b0;
            completion_first <= 1'b0;
        end else if (rd_rsp_valid && rd_rsp_first) begin
            completion_keep <= 4'b1111;
            completion_last <= rd_rsp_last;
            completion_first <= 1'b1;
            completion_data[31:0] <= {
                22'b0100101000000000000000,
                rd_rsp_dwlen
            };
            completion_data[63:32] <= {
                pcie_id[7:0],
                pcie_id[15:8],
                4'b0,
                rd_rsp_byte_count
            };
            completion_data[95:64] <= {
                rd_rsp_reqid,
                rd_rsp_tag,
                1'b0,
                rd_rsp_lowaddr
            };
            completion_data[127:96] <= rd_rsp_data_wire_order;
        end else begin
            completion_last <= rd_rsp_valid && rd_rsp_last;
            completion_keep <= completion_valid
                ? (rd_rsp_valid ? 4'b0001 : 4'b0000)
                : (rd_rsp_valid
                    ? ((completion_keep << 1) | 1'b1)
                    : completion_keep);
            completion_first <= 1'b0;
            if (rd_rsp_valid) begin
                if (completion_valid || !completion_keep[0])
                    completion_data[31:0] <= rd_rsp_data_wire_order;
                if (!completion_keep[1])
                    completion_data[63:32] <= rd_rsp_data_wire_order;
                if (!completion_keep[2])
                    completion_data[95:64] <= rd_rsp_data_wire_order;
                if (!completion_keep[3])
                    completion_data[127:96] <= rd_rsp_data_wire_order;
            end
        end
    end

    fifo_134_134_clk1_bar_rdrsp i_fifo_134_134_clk1_bar_rdrsp(
        .srst       ( rst                   ),
        .clk        ( clk                   ),
        .din        ( {
            completion_first,
            completion_last,
            completion_keep,
            completion_data
        } ),
        .wr_en      ( completion_valid      ),
        .rd_en      ( tlps_out.tready       ),
        .dout       ( {
            tlps_out.tuser[0],
            tlps_out.tlast,
            tlps_out.tkeepdw,
            tlps_out.tdata
        } ),
        .full       (                       ),
        .empty      (                       ),
        .prog_empty ( rd3_enable            ),
        .valid      ( tlps_out.tvalid       )
    );

    assign tlps_out.tuser[1] = tlps_out.tlast;
    assign tlps_out.tuser[8:2] = 7'b0;

    reg [10:0] packet_count = 11'd0;
    wire packet_count_dec = tlps_out.tvalid && tlps_out.tlast;
    wire packet_count_inc = completion_valid && completion_last;
    wire [10:0] packet_count_next =
        packet_count + packet_count_inc - packet_count_dec;
    assign tlps_out.has_data = (packet_count_next > 0);

    always @(posedge clk)
        packet_count <= rst ? 11'd0 : packet_count_next;

endmodule
