`timescale 1ns/1ps
module svmvisor_tx_guard_tb;
    import svmvisor_pcie_pkg::*;
    logic clk=0; always #5 clk=~clk;
    logic rst=1;
    read_token_t token;
    logic token_valid=0,token_ready;
    logic [63:0] in_data=0,out_data;
    logic [7:0] in_keep=0,out_keep;
    logic in_valid=0,in_last=0,in_ready,out_valid,out_last,out_ready=1,violation,idle;
    integer emitted=0;
    svmvisor_tx_guard dut(.*);
    always @(posedge clk) if(out_valid && out_ready) emitted++;
    task reset_guard;
        @(negedge clk); rst=1; token_valid=0; in_valid=0;
        repeat(3) @(negedge clk); rst=0; emitted=0;
    endtask
    task arm;
        @(negedge clk); token='0; token.completer=16'h0200; token.requester=16'h1234;
        token.tag=8'h56; token.words=1; token.bytes_left=4; token_valid=1;
        @(negedge clk); token_valid=0;
    endtask
    task beat(input logic[63:0] d,input logic last,input logic[7:0] keep);
        @(negedge clk); in_data=d; in_last=last; in_keep=keep; in_valid=1;
        do @(posedge clk); while(!in_ready);
        @(negedge clk); in_valid=0;
    endtask
    task bad_packet(input logic[31:0] h0,input logic[31:0] h1,input logic[31:0] h2);
        reset_guard(); arm(); beat({h1,h0},0,8'hff); beat({32'hdeadbeef,h2},1,8'hff);
        repeat(5) @(negedge clk);
        if(!violation || emitted!=0) $fatal(1,"forbidden packet escaped");
    endtask
    initial begin
        bad_packet(32'h00000001,32'h02000004,32'h12345600); // requester read
        bad_packet(32'h40000001,32'h02000004,32'h12345600); // requester write
        bad_packet(32'h30000001,32'h02000004,32'h12345600); // message
        bad_packet(32'h4a000001,32'h02000004,32'h12345700); // wrong tag
        bad_packet(32'h4a000001,32'h02000004,32'h99995600); // wrong requester
        bad_packet(32'h4a000002,32'h02000004,32'h12345600); // excess length
        bad_packet(32'h4a000001,32'h02000008,32'h12345600); // excess byte count
        bad_packet(32'h4a000001,32'h03000004,32'h12345600); // wrong completer
        bad_packet(32'h4a000001,32'h02000004,32'h12345604); // wrong lower address
        reset_guard(); arm(); // incomplete second header/payload
        beat(64'h020000044a000001,1,8'hff);
        repeat(5) @(negedge clk); if(!violation || emitted!=0) $fatal(1,"truncated TX");
        reset_guard(); arm(); // buffer overflow must be drained, never forwarded
        beat(64'h020000044a000001,0,8'hff);
        for(integer n=0;n<12;n++) beat(64'hdeadbeef12345600,n==11,8'hff);
        repeat(5) @(negedge clk); if(!violation || emitted!=0) $fatal(1,"overlong TX");
        reset_guard(); arm(); out_ready=0;
        beat(64'h020000044a000001,0,8'hff); beat(64'hdeadbeef12345600,1,8'hff);
        wait(out_valid); reset_guard(); out_ready=1;
        repeat(5) @(negedge clk); if(out_valid || emitted!=0) $fatal(1,"reset retained TX");
        reset_guard(); // no inbound request
        beat(64'h020000044a000001,0,8'hff); beat(64'hdeadbeef12345600,1,8'hff);
        repeat(5) @(negedge clk); if(!violation || emitted!=0) $fatal(1,"unsolicited TX");
        reset_guard(); arm(); beat(64'h020000044a000001,0,8'hff); beat(64'hdeadbeef12345600,1,8'hff);
        wait(idle); if(emitted!=2 || violation) $fatal(1,"valid completion rejected");
        beat(64'h020000044a000001,0,8'hff); beat(64'hdeadbeef12345600,1,8'hff);
        repeat(5) @(negedge clk); if(!violation || emitted!=2) $fatal(1,"duplicate TX");
        $display("PASS: TX guard requester/message/mismatch/excess/unsolicited/duplicate rejection"); $finish;
    end
    initial begin #100000; $fatal(1,"timeout"); end
endmodule
