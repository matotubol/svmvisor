`timescale 1ns/1ps
module svmvisor_percpu_snapshot_tb;
    logic user_clk=0,board_clk=0,run_user=1;
    always #8 if(run_user) user_clk=~user_clk;
    always #5 board_clk=~board_clk;
    logic transaction_reset=0,write_valid=0,bad_alignment=0;
    logic [11:0] write_address=0;
    logic [31:0] write_data=0;
    logic [3:0] write_be=15;
    logic drck=0,capture=0,shift=0,selected=0,tdi=0;
    wire tdo;
    svmvisor_percpu_snapshot #(.FPGA_BUILD_ID(64'h0123456789abcdef),.ROM_BUILD_ID(64'h123456789abcdef0)) dut(.*);
    task wr(input integer slot,input integer wordno,input logic [31:0] value);
        @(negedge user_clk); write_valid=1; write_address='h600+slot*80+wordno*4; write_data=value;
        @(negedge user_clk); write_valid=0;
    endtask
    task record(input integer slot,input integer seq,input bit fault);
        wr(slot,0,seq); wr(slot,1,32'h100|(fault ? 32'h10000 : 0)|8'h04);
        for(integer i=2;i<19;i++) wr(slot,i,seq*256+i);
        wr(slot,19,seq);
    endtask
    task scan(input integer bank,output logic [1023:0] f);
        selected=1; capture=1; #500; drck=1; #500; drck=0; capture=0; shift=1;
        for(integer i=0;i<1024;i++) begin
            tdi=i<6 ? ((bank>>i)&1) : 0; f[i]=tdo;
            #500; drck=1; #500; drck=0;
        end
        shift=0; selected=0; #10000;
    endtask
    function automatic bit crc_ok(input logic [1023:0] f);
        logic [31:0] c; c=32'hffffffff;
        for(integer i=0;i<992;i++) c=(c[0]^f[i]) ? (c>>1)^32'hedb88320 : c>>1;
        return f[1023:992]==~c;
    endfunction
    task check(input logic [1023:0] f,input integer bank,input integer seq);
        if(!crc_ok(f) || f[31:0]!=32'h55504353 || f[63:32]!=32'h04000001 ||
            f[127:64]!=64'h0123456789abcdef || f[191:128]!=64'h123456789abcdef0 ||
            f[197:192]!=bank || !f[198] || f[287:256]!=seq || f[991:864]!=0)
            $fatal(1,"bad bank %d seq %d: %h",bank,seq,f);
    endtask
    logic [1023:0] f,g;
    integer fd;
    initial begin
        // Interleaved CPU writes must never share staging/commit state.
        wr(0,0,1); wr(31,0,2);
        for(integer i=1;i<19;i++) begin
            wr(0,i,i==1 ? 32'h104 : i);
            wr(31,i,i==1 ? 32'h104 : 100+i);
        end
        wr(31,19,2); wr(0,19,1);
        #100000; repeat(4) scan(31,f); check(f,31,2);
        repeat(4) scan(0,f); check(f,0,1);
        // A first terminal fault is durable even while later progress continues.
        record(0,3,1); record(0,4,1); record(0,5,0);
        #100000; repeat(4) scan(32,f); check(f,32,3);
        repeat(4) scan(0,f); check(f,0,5);
        fd=$fopen("percpu-valid.hex","w"); $fdisplay(fd,"%0256h",f); $fclose(fd);
        // A reset halfway through staging cancels it, preserves prior evidence.
        wr(0,0,99); wr(0,1,32'h104);
        @(negedge user_clk); transaction_reset=1;
        repeat(3) @(negedge user_clk); transaction_reset=0;
        for(integer i=2;i<19;i++) wr(0,i,i);
        wr(0,19,99); #100000; repeat(4) scan(0,f); check(f,0,5);
        if(!f[225]) $fatal(1,"incomplete commit was not diagnosed");
        // Publication during a scan never changes its captured record.
        fork
            scan(0,f);
            begin #100000; record(0,6,0); end
        join
        check(f,0,5); repeat(4) scan(0,f); check(f,0,6);
        // PCIe/user clock loss cannot prevent retained board/JTAG readout.
        @(negedge user_clk); run_user=0;
        repeat(4) scan(32,f); check(f,32,3);
        repeat(4) scan(31,f); check(f,31,2);
        scan(31,g); if(f!==g) $fatal(1,"stationary retained record changed");
        $display("PASS: perCPU atomic interleaving, sticky first fault, incomplete commit/reset retention, midscan publication, stopped PCIe clock independent readout");
        $finish;
    end
    initial begin #100000000; $fatal(1,"perCPU timeout"); end
endmodule
