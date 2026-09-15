`timescale 1ns/1ps
module svmvisor_bypass_tb;
    logic clk=0; always #5 clk=~clk;
    logic perst_n=0,switch_n=0;
    wire bypass,core_release;
    svmvisor_bypass dut(.*);
    initial begin
        repeat(3) @(negedge clk); perst_n=1;
        repeat(70) @(negedge clk);
        if(!bypass || core_release) $fatal(1,"asserted bypass released PCIe");
        switch_n=1; repeat(70) @(negedge clk);
        if(!bypass || core_release) $fatal(1,"live switch defeated bypass latch");
        perst_n=0; repeat(3) @(negedge clk); perst_n=1;
        repeat(60) begin @(negedge clk); if(core_release) $fatal(1,"early core release"); end
        repeat(10) @(negedge clk);
        if(bypass || !core_release) $fatal(1,"normal release failed");
        switch_n=0; repeat(70) @(negedge clk);
        if(bypass || !core_release) $fatal(1,"latch changed without reset");
        perst_n=0; #1;
        if(core_release) $fatal(1,"PERST did not stop core");
        $display("PASS: bypass sampling, immutable state, PERST reset"); $finish;
    end
    initial begin #10000; $fatal(1,"timeout"); end
endmodule
