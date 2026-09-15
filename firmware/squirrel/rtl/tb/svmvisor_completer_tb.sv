`timescale 1ns/1ps
module svmvisor_completer_tb;
    import svmvisor_pcie_pkg::*;
    logic clk=0; always #5 clk=~clk;
    logic rst=1;
    logic [63:0] rx_data=0;
    logic [7:0] rx_keep=0;
    logic [21:0] rx_user=0;
    logic rx_valid=0,rx_last=0,rx_ready;
    read_token_t token;
    logic token_valid,token_ready;
    logic [63:0] built_data,out_data;
    logic [7:0] built_keep,out_keep;
    logic built_valid,built_last,built_ready,out_valid,out_last,out_ready=0;
    logic fault,rx_fault,busy,idle;
    svmvisor_completer #(.ROM_FILE("")) dut(
        .clk,.rst,.hardware_status(32'b0),.completer_id(16'h0200),.rx_data,.rx_keep,.rx_user,.rx_valid,.rx_last,.rx_ready,
        .token,.token_valid,.token_ready,.tx_data(built_data),.tx_keep(built_keep),
        .tx_valid(built_valid),.tx_last(built_last),.tx_ready(built_ready),.tx_fault(fault),.rx_fault,.busy);
    svmvisor_tx_guard guard(.clk,.rst,.token,.token_valid,.token_ready,
        .in_data(built_data),.in_keep(built_keep),.in_valid(built_valid),.in_last(built_last),.in_ready(built_ready),
        .out_data,.out_keep,.out_valid,.out_last,.out_ready,.violation(fault),.idle);
    task beat(input logic [63:0] d,input logic [7:0] keep,input logic last,input logic [7:0] bar);
        @(negedge clk); rx_data=d; rx_keep=keep; rx_last=last; rx_user={12'b0,bar,2'b0}; rx_valid=1;
        do @(posedge clk); while (!rx_ready);
        @(negedge clk); rx_valid=0;
    endtask
    task read_request(input integer addr,input integer words,input logic [7:0] be,input logic four,input logic [7:0] bar);
        beat({16'h1234,8'h56,be,(four ? 32'h20000000 : 32'h00000000) | (words & 1023)},8'hff,0,bar);
        if (four) beat({32'(addr),32'h00000001},8'hff,1,bar);
        else beat({32'b0,32'(addr)},8'h0f,1,bar);
    endtask
    task completion(input integer addr,input integer words,input integer bytes_left,input integer lower,input logic rom_data_expected,input logic [31:0] single_data);
        logic [31:0] got[0:19];
        logic [63:0] held;
        logic [7:0] held_keep;
        logic held_last;
        logic done;
        integer n;
        wait(out_valid); @(negedge clk);
        held=out_data; held_keep=out_keep; held_last=out_last;
        repeat(4) begin @(posedge clk); #1;
            if (!out_valid || out_data !== held || out_keep !== held_keep || out_last !== held_last) $fatal(1,"backpressure stability");
        end
        @(negedge clk); out_ready=1; n=0; done=0;
        while (!done) begin
            @(posedge clk);
            if (out_valid) begin
                got[n]=out_data[31:0]; n++;
                if (out_keep==8'hff) begin got[n]=out_data[63:32]; n++; end
                else if(out_keep!=8'h0f || !out_last) $fatal(1,"keep");
                done=out_last;
            end
        end
        @(negedge clk); out_ready=0;
        if(n!=words+3 || got[0] !== (32'h4a000000 | words) ||
            got[1] !== (32'h02000000 | (bytes_left & 4095)) || got[2] !== (32'h12345600 | lower))
            $fatal(1,"completion headers n=%0d h=%h/%h/%h",n,got[0],got[1],got[2]);
        for(integer j=0;j<words;j++) begin
            logic [31:0] expected_data;
            expected_data=rom_data_expected ? 32'ha5000000+((addr>>2)+j) : single_data;
            if(got[j+3] !== {expected_data[7:0],expected_data[15:8],expected_data[23:16],expected_data[31:24]})
                $fatal(1,"payload at %0d got %h expected %h",j,got[j+3],expected_data);
        end
    endtask
    initial begin
        #1; for(integer j=0;j<2048;j++) dut.rom[j]=32'ha5000000+j;
        repeat(4) @(negedge clk); rst=0;
        // An unaligned posted write is rejected before the journal write port.
        beat({32'h1234560f,32'h40000001},8'hff,0,8'h01);
        beat({32'h01000000,32'h00000041},8'hff,1,8'h01);
        wait(!busy);
        read_request('h24,1,8'h0f,0,8'h01); completion('h24,1,4,'h24,0,10);
        read_request('h3c,20,8'hff,0,8'h40);
        completion('h3c,1,80,'h3c,1,0);
        completion('h40,16,76,'h40,1,0);
        completion('h80,3,12,0,1,0);
        read_request('h040,1,8'h08,1,8'h40); completion('h040,1,1,'h43,1,0);
        read_request(0,1,8'h00,0,8'h40); completion(0,1,1,0,1,0);
        read_request(0,1024,8'hff,0,8'h40);
        for(integer j=0;j<64;j++) completion(j*64,16,4096-j*64,(j*64)&127,1,0);
        // The upper ROM half has distinct data and survives split completions.
        read_request('h1000,1024,8'hff,1,8'h40);
        for(integer j=0;j<64;j++) completion('h1000+j*64,16,4096-j*64,(j*64)&127,1,0);
        read_request('h1ffc,1,8'h0f,0,8'h40); completion('h1ffc,1,4,'h7c,1,0);
        // Upper-half writes remain ignored, and the next lower-half read must
        // not inherit the previous ROM page bit.
        beat({32'h1234560f,32'h40000001},8'hff,0,8'h40);
        beat({32'h00000000,32'h00001040},8'hff,1,8'h40);
        read_request('h1040,1,8'h0f,0,8'h40); completion('h1040,1,4,'h40,1,0);
        read_request('h0040,1,8'h0f,0,8'h40); completion('h0040,1,4,'h40,1,0);
        // Partial writes to journal staging are rejected; no posted reply.
        beat({32'h12345605,32'h40000001},8'hff,0,8'h01);
        beat({32'h11223344,32'h00000040},8'hff,1,8'h01);
        wait(!busy); repeat(5) @(posedge clk);
        if(out_valid) $fatal(1,"posted write generated TX");
        read_request('h040,1,8'h0f,0,8'h01); completion('h040,1,4,'h40,0,32'h00000000);
        // Writes to ROM have no effect.
        beat({32'h1234560f,32'h40000001},8'hff,0,8'h40);
        beat({32'h00000000,32'h00000040},8'hff,1,8'h40);
        read_request('h040,1,8'h0f,0,8'h40); completion('h040,1,4,'h40,1,0);
        // Malformed length is drained without TX or a BAR write.
        beat({32'h1234560f,32'h40000002},8'hff,0,8'h01);
        beat({32'hffffffff,32'h00000040},8'hff,1,8'h01);
        wait(!busy); repeat(10) @(posedge clk);
        if(out_valid || !rx_fault || fault) $fatal(1,"malformed handling");
        read_request('h040,1,8'h0f,0,8'h01); completion('h040,1,4,'h40,0,32'h00000000);
        // Poisoned posted packet must not alter a diagnostic word.
        beat({32'h1234560f,32'h40004001},8'hff,0,8'h01);
        beat({32'hffffffff,32'h00000040},8'hff,1,8'h01);
        read_request('h040,1,8'h0f,0,8'h01); completion('h040,1,4,'h40,0,32'h00000000);
        // Maximum 128-byte write with a 64-bit address, fully buffered first.
        beat({32'h123456ff,32'h60000020},8'hff,0,8'h01);
        beat({32'h00000040,32'h00000001},8'hff,0,8'h01);
        for(integer j=0;j<16;j++) beat({32'h78563412,32'h78563412},8'hff,j==15,8'h01);
        read_request('h040,1,8'h0f,0,8'h01); completion('h040,1,4,'h40,0,32'h12345678);
        read_request('h05c,1,8'h0f,0,8'h01); completion('h05c,1,4,'h5c,0,32'h12345678);
        // A read crossing a PCIe 4 KiB boundary is malformed and must not wrap.
        read_request('hffc,2,8'hff,0,8'h40);
        wait(!busy); repeat(5) @(posedge clk);
        if(fault || out_valid) $fatal(1,"unexpected guard fault or aperture-wrap reply");
        read_request('h1ffc,2,8'hff,0,8'h40);
        wait(!busy); repeat(5) @(posedge clk);
        if(fault || out_valid) $fatal(1,"upper ROM aperture-wrap reply");
        // Publish through actual RX/TX framing, then reset the transaction path.
        // One 9-DWORD posted write carries eight fields followed by COMMIT_SEQ.
        beat({32'h123456ff,32'h40000009},8'hff,0,8'h01);
        beat({32'h2a000000,32'h00000040},8'hff,0,8'h01);
        for(integer j=0;j<3;j++) beat({32'h2a000000,32'h2a000000},8'hff,0,8'h01);
        beat({32'h2a000000,32'h2a000000},8'hff,1,8'h01);
        wait(!busy);
        read_request('h2c,1,8'h0f,0,8'h01); completion('h2c,1,4,'h2c,0,42);
        read_request('h80,8,8'hff,0,8'h01); completion('h80,8,32,0,0,42);
        wait(!busy); @(negedge clk); rst=1;
        repeat(4) @(negedge clk); rst=0;
        read_request('h2c,1,8'h0f,0,8'h01); completion('h2c,1,4,'h2c,0,42);
        // The earlier maximum-size write committed slot 0; this event is slot 1.
        read_request('h120,8,8'hff,0,8'h01); completion('h120,8,32,'h20,0,42);
        $display("PASS: completer framing, guarded replies, journal commit and reset retention");
        $finish;
    end
    initial begin #2000000; $fatal(1,"timeout"); end
endmodule

