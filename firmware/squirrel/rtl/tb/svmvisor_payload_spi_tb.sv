`timescale 1ns/1ps
module svmvisor_payload_spi_tb;
    logic clk=0; always #8 clk=~clk;
    logic rst=1,cancel=0,request=0,configuration_done=0;
    logic [19:0] address=0;
    wire ready,response_valid,response_error;
    wire [31:0] response_data;
    wire spi_clk,spi_cs_n,spi_mosi,spi_miso;
    svmvisor_payload_spi dut(.*);
    // Include Q->STARTUP route4.124ns + primitive6.7ns + PCB1ns,
    // rounded upward, in the physical clock seen by the independent flash.
    wire physical_sck, flash_data;
    assign #12.8 physical_sck=spi_clk;
    // Flash model already adds8ns tV; add returnPCB1ns + inputpath4.5ns.
    assign #5.5 spi_miso=flash_data;
    svmvisor_spi_flash_model flash(.spi_clk(physical_sck),.spi_cs_n,.spi_mosi,.spi_miso(flash_data));
    task read_word(input logic [19:0] a,input logic bad);
        integer cycles,previous_count;
        wait(ready); @(negedge clk); address=a; request=1; previous_count=flash.transactions;
        @(negedge clk); request=0; cycles=0;
        while(!response_valid) begin @(negedge clk); cycles++; if(cycles>300) $fatal(1,"unbounded SPI read"); end
        if(response_error !== bad || !spi_cs_n || spi_clk) $fatal(1,"bad completion/idle pins");
        if(bad) begin
            if(response_data !== 32'hffffffff || flash.transactions!=previous_count) $fatal(1,"unaligned read touched flash");
        end else if(response_data !== flash.word_at(32'h00400000+{12'b0,a}))
            $fatal(1,"wrong bytes at %h: %h",a,response_data);
    endtask
    integer sampled_bits=0;
    always @(negedge spi_cs_n) sampled_bits=0;
    always @(negedge clk) begin
        if(dut.state==dut.SHIFT && dut.spi_clk && !dut.half_cycle && dut.bit_count>=40) sampled_bits++;
    end
    always @(posedge response_valid) if(!response_error && sampled_bits!=32)
        $fatal(1,"capture enable did not sample exactly32 bits: %0d",sampled_bits);
    // During an active command MOSI must change only with falling drive SCK.
    always @(spi_mosi) if(!spi_cs_n && spi_clk) $fatal(1,"MOSI changed in rising/high phase");
    initial begin
        repeat(4) @(negedge clk); rst=0;
        repeat(30) begin
            @(negedge clk);
            if(ready || spi_clk || !spi_cs_n || response_valid) $fatal(1,"SPI started before EOS");
        end
        configuration_done=1;
        wait(ready); if(flash.transactions!=0) $fatal(1,"prime selected flash");
        read_word(0,0); read_word('h12344,0); read_word('hffffc,0);
        read_word(1,1); read_word('hfffff,1);
        // Abort both command and data phases, then require a fresh prime/read.
        for(integer phase=0;phase<2;phase++) begin
            wait(ready); @(negedge clk); request=1; address=0;
            @(negedge clk); request=0;
            repeat(phase==0 ? 12 : 205) @(negedge clk);
            cancel=1; @(negedge clk); cancel=0;
            if(!spi_cs_n || spi_clk || response_valid) $fatal(1,"cancel did not release flash");
            read_word('habcd0,0);
        end
        wait(ready); @(negedge clk); request=1;
        @(negedge clk); request=0;
        repeat(188) @(negedge clk); rst=1;
        @(negedge clk); rst=0;
        if(!spi_cs_n || spi_clk || response_valid) $fatal(1,"reset retained response");
        read_word('hfedc0,0);
        $display("PASS: payload SPI bounded command, data timing, slot endpoints, alignment, cancel and reset"); $finish;
    end
    initial begin #100000; $fatal(1,"timeout"); end
endmodule
