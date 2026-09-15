`timescale 1ns/1ps
module svmvisor_snapshot_tb;
    logic user_clk=0,board_clk=0,run_user=1;
    always #8 if(run_user) user_clk=~user_clk;
    always #5 board_clk=~board_clk;
    logic rst=0,wr_valid=0;
    logic [11:0] wr_addr=0;
    logic [31:0] wr_data=0;
    wire [255:0] record_data;
    wire [31:0] reads,writes,errors;
    logic [31:0] status=0;
    logic perst_n=1,link_up=0,drck=0,capture=0,shift=0,selected=0;
    wire tdo;
    svmvisor_journal journal(.clk(user_clk),.transaction_reset(rst),
        .write_valid(wr_valid),.write_address(wr_addr),.write_data(wr_data),.write_be(4'hf),
        .rom_read(1'b0),.bar_write_packet(wr_valid),.rx_fault(1'b0),.tx_fault(1'b0),.bad_alignment(1'b0),
        .hardware_status(status),.read_address(12'b0),.read_data(),.committed_record(record_data),
        .commit_pulse(),.rom_count(reads),.write_count(writes),.errors);
    svmvisor_snapshot #(.FPGA_BUILD_ID(64'h0807060504030201),.ROM_BUILD_ID(64'h1817161514131211)) dut(
        .user_clk,.board_clk,.committed_record(record_data),.hardware_status(status),
        .rom_count(reads),.write_count(writes),.perst_n,.link_up,.drck,.capture,.shift,.selected,.tdo);
    task wr(input integer a,input logic [31:0] d);
        @(negedge user_clk); wr_valid=1; wr_addr=a; wr_data=d;
        @(negedge user_clk); wr_valid=0;
    endtask
    task event_record(input integer seq);
        wr('h40,seq); wr('h44,32'h12345678); wr('h48,32'habcd0001); wr('h4c,32'h98760002);
        wr('h50,32'hfedcba98); wr('h54,32'h76543210); wr('h58,32'h1234); wr('h5c,32'h00170009);
        wr('h60,seq);
    endtask
    task scan(output logic [511:0] data);
        selected=1; capture=1; #13; drck=1; #13; drck=0; capture=0; shift=1;
        for(integer i=0;i<512;i++) begin
            data[i]=tdo; #13; drck=1; #13; drck=0;
        end
        shift=0; selected=0; #2000;
    endtask
    function automatic logic crc_ok(input logic [511:0] f);
        logic [31:0] c;
        c=32'hffffffff;
        for(integer i=0;i<480;i++) c=(c[0]^f[i]) ? (c>>1)^32'hedb88320 : c>>1;
        return f[511:480]==~c;
    endfunction
    task check(input logic [511:0] f,input integer seq);
        if(!crc_ok(f) || f[31:0]!=32'h50414e53 || f[63:32]!=32'h02000001 ||
            f[127:64]!=64'h0807060504030201 || f[191:128]!=64'h1817161514131211 ||
            f[255:224]!=seq) $fatal(1,"bad frame or sequence: %h",f);
        if(seq!=0 && (f[223:192]!=32'h12345678 || f[287:256]!=32'h00170009 ||
            f[319:288]!=32'h1234 || f[383:320]!=64'h76543210fedcba98)) $fatal(1,"field ordering");
    endtask
    logic [511:0] a,b;
    integer fd;
    initial begin
        #400; scan(a); scan(a); check(a,0);
        event_record(42); link_up=1; status=32'h301; #3000;
        repeat(3) scan(a); check(a,42);
        if(a[384]!=1 || a[393:392]!=3) $fatal(1,"status wiring");
        fd=$fopen("snapshot-valid.hex","w"); $fdisplay(fd,"%0128h",a); $fclose(fd);
        // Uncommitted staging never enters the snapshot.
        wr('h40,99); wr('h5c,32'hffffeeee); #3000;
        repeat(3) scan(a); check(a,42);
        // A commit during shifting cannot tear the frame captured at its start.
        fork
            scan(a);
            begin #1000; event_record(43); end
        join
        check(a,42); repeat(3) scan(a); check(a,43);
        // Transaction reset changes neither committed record nor crossing state.
        @(negedge user_clk); rst=1; repeat(5) @(negedge user_clk); rst=0;
        #3000; repeat(3) scan(a); check(a,43);
        // Stop the PCIe clock, then change PERST/link in the board domain.
        @(negedge user_clk); run_user=0; perst_n=0; link_up=0;
        #3000; repeat(3) scan(a); check(a,43);
        if(a[384]!=0 || a[391]!=1) $fatal(1,"board status stopped with PCIe clock");
        scan(b); if(a!==b) $fatal(1,"stationary consecutive reads differ");
        // Abandon a scan, then Capture-DR starts a fresh immutable copy.
        capture=1; selected=1; #13; drck=1; #13; drck=0; capture=0; selected=0;
        scan(b); if(a!==b) $fatal(1,"abandoned scan damaged evidence");
        $display("PASS: snapshot CRC/field order, atomic crossing, hidden staging, mid-scan commit, stopped clock/reset retention");
        $finish;
    end
    initial begin #1000000; $fatal(1,"snapshot timeout"); end
endmodule
