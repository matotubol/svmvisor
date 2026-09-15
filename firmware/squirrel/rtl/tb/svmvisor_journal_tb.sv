`timescale 1ns/1ps
module svmvisor_journal_tb;
    logic clk=0; always #5 clk=~clk;
    logic transaction_reset=0,write_valid=0,rom_read=0,bar_write_packet=0,rx_fault=0,tx_fault=0;
    logic [11:0] write_address=0,read_address=0;
    logic [31:0] write_data=0,read_data,rom_count,write_count,errors;
    logic [3:0] write_be=15;
    logic [255:0] committed_record;
    logic commit_pulse;
    svmvisor_journal dut(.clk,.transaction_reset,.write_valid,.write_address,.write_data,.write_be,
        .rom_read,.bar_write_packet,.rx_fault,.tx_fault,.bad_alignment(1'b0),.hardware_status(32'b0),
        .read_address,.read_data,.committed_record,.commit_pulse,.rom_count,.write_count,.errors);
    task wr(input integer address,input logic [31:0] data,input logic [3:0] be=15);
        @(negedge clk); write_address=address; write_data=data; write_be=be; write_valid=1;
        @(negedge clk); write_valid=0;
    endtask
    task expect_word(input integer address,input logic [31:0] data);
        read_address=address; #1;
        if(read_data !== data) $fatal(1,"BAR[%h]: %h != %h",address,read_data,data);
    endtask
    task stage(input integer sequence_number);
        for(integer i=0;i<8;i++) wr('h40+i*4,sequence_number+i);
    endtask
    initial begin
        #1; expect_word(0,32'h4a4d5653); expect_word('h28,0);
        stage(10); expect_word('h2c,0); expect_word('h80,0);
        wr('h60,11); expect_word('h2c,0);
        if(!errors[2]) $fatal(1,"mismatching commit accepted");
        stage(10); wr('h60,10);
        for(integer i=0;i<8;i++) begin expect_word('h80+i*4,10+i); expect_word('h100+i*4,10+i); end
        expect_word('h28,32'h00010001);
        wr('h60,10); expect_word('h28,32'h00010001); // no stale staging replay
        stage(20); wr('h44,99,3); wr('h60,20); expect_word('h2c,10);
        stage(20); wr('h41,99); wr('h60,20); expect_word('h2c,10);
        stage(20); wr('h100,99); wr('h60,20); expect_word('h2c,10);
        if(errors[1:0] != 3) $fatal(1,"invalid write errors missing");
        stage(20);
        @(negedge clk); transaction_reset=1;
        repeat(4) @(negedge clk);
        transaction_reset=0;
        wr('h60,20); expect_word('h2c,10); expect_word('h100,10);
        // Wrap history, saturate count, preserve every word of latest event.
        for(integer n=11;n<=80;n++) begin stage(n); wr('h60,n); end
        expect_word('h28,32'h00200007); expect_word('h2c,80);
        for(integer i=0;i<8;i++) begin
            expect_word('h80+i*4,80+i);
            expect_word('h100+6*32+i*4,80+i);
            expect_word('h100+7*32+i*4,49+i);
        end
        expect_word('h900,0); expect_word('hffc,0); expect_word('h60,0);
        @(negedge clk); rom_read=1; bar_write_packet=1; tx_fault=1;
        @(negedge clk); rom_read=0; bar_write_packet=0; tx_fault=0; transaction_reset=1;
        repeat(3) @(negedge clk); transaction_reset=0;
        expect_word('h1c,1); expect_word('h20,1); expect_word('h2c,80);
        if(!errors[4]) $fatal(1,"fault did not survive reset");
        // Boundary injection checks saturation without billions of simulated clocks.
        @(negedge clk); dut.rom_count=32'hfffffffe; dut.write_count=32'hfffffffe;
        rom_read=1; bar_write_packet=1;
        repeat(4) @(negedge clk); rom_read=0; bar_write_packet=0;
        expect_word('h1c,32'hffffffff); expect_word('h20,32'hffffffff);
        $display("PASS: journal atomicity, invalid commits, reset retention, wrap, protection, saturation");
        $finish;
    end
    initial begin #100000; $fatal(1,"timeout"); end
endmodule
