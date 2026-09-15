`timescale 1ns/1ps
module svmvisor_payload_completer_tb;
    import svmvisor_pcie_pkg::*;
    logic clk=0; always #8 clk=~clk;
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
    wire payload_request,payload_cancel,payload_ready,payload_response_valid,payload_response_error;
    wire [19:0] payload_address;
    wire [31:0] payload_response_data;
    wire spi_clk,spi_cs_n,spi_mosi,spi_miso;
    logic [15:0] requester=16'h1234;
    logic [7:0] request_tag=8'h56;
    svmvisor_payload_spi reader(.clk,.rst,.configuration_done(1'b1),.cancel(payload_cancel),.request(payload_request),
        .address(payload_address),.ready(payload_ready),.response_valid(payload_response_valid),
        .response_error(payload_response_error),.response_data(payload_response_data),.spi_clk,.spi_cs_n,.spi_mosi,.spi_miso);
    // Include Q->STARTUP route4.124ns + primitive6.7ns + PCB1ns,
    // rounded upward, in the physical clock seen by the independent flash.
    wire physical_sck, flash_data;
    assign #12.8 physical_sck=spi_clk;
    // Flash model already adds8ns tV; add returnPCB1ns + inputpath4.5ns.
    assign #5.5 spi_miso=flash_data;
    svmvisor_spi_flash_model flash(.spi_clk(physical_sck),.spi_cs_n,.spi_mosi,.spi_miso(flash_data));
    svmvisor_completer #(.ROM_FILE(""),.PAYLOAD_ENABLED(1),.ROM_BYTES(65536)) dut(
        .clk,.rst,.hardware_status(32'b0),.completer_id(16'h0200),.rx_data,.rx_keep,.rx_user,.rx_valid,.rx_last,.rx_ready,
        .token,.token_valid,.token_ready,.tx_data(built_data),.tx_keep(built_keep),
        .tx_valid(built_valid),.tx_last(built_last),.tx_ready(built_ready),.tx_fault(fault),.rx_fault,.busy,
        .payload_request,.payload_cancel,.payload_address,.payload_ready,.payload_response_valid,.payload_response_error,.payload_response_data);
    svmvisor_tx_guard guard(.clk,.rst,.token,.token_valid,.token_ready,
        .in_data(built_data),.in_keep(built_keep),.in_valid(built_valid),.in_last(built_last),.in_ready(built_ready),
        .out_data,.out_keep,.out_valid,.out_last,.out_ready,.violation(fault),.idle);
    task beat(input logic [63:0] d,input logic [7:0] keep,input logic last,input logic [7:0] bar);
        @(negedge clk); rx_data=d; rx_keep=keep; rx_last=last; rx_user={12'b0,bar,2'b0}; rx_valid=1;
        do @(posedge clk); while (!rx_ready);
        @(negedge clk); rx_valid=0;
    endtask
    task read_request(input integer addr,input integer words,input logic [7:0] be,input logic four,input logic [7:0] bar);
        beat({requester,request_tag,be,(four ? 32'h20000000 : 32'h00000000) | (words & 1023)},8'hff,0,bar);
        if (four) beat({32'(addr),32'h00000001},8'hff,1,bar);
        else beat({32'b0,32'(addr)},8'h0f,1,bar);
    endtask
    task completion(input integer addr,input integer words,input integer bytes_left,input integer lower,input logic [1:0] rom_data_expected,input logic [31:0] single_data);
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
            got[1] !== (32'h02000000 | (bytes_left & 4095)) || got[2] !== ({requester,request_tag,8'b0} | lower))
            $fatal(1,"completion headers n=%0d h=%h/%h/%h",n,got[0],got[1],got[2]);
        for(integer j=0;j<words;j++) begin
            logic [31:0] expected_data;
            expected_data=rom_data_expected==1 ? 32'ha5000000+((addr>>2)+j) :
                rom_data_expected==2 ? flash.word_at(32'h00400000+addr+4*j) : single_data;
            if(got[j+3] !== {expected_data[7:0],expected_data[15:8],expected_data[23:16],expected_data[31:24]})
                $fatal(1,"payload at %0d got %h expected %h",j,got[j+3],expected_data);
        end
    endtask
    task reset_transaction;
        @(negedge clk); rst=1; out_ready=0;
        repeat(4) @(negedge clk); rst=0;
    endtask
    task unsupported;
        logic [31:0] got[0:3];
        integer n;
        wait(out_valid); @(negedge clk); out_ready=1; n=0;
        do begin
            @(posedge clk);
            if(out_valid) begin
                got[n]=out_data[31:0]; n++;
                if(out_keep==8'hff) begin got[n]=out_data[63:32]; n++; end
            end
        end while(!(out_valid && out_last));
        @(negedge clk); out_ready=0;
        if(n!=3 || got[0]!==32'h0a000000 || got[1]!==32'h02002000 || got[2]!=={requester,request_tag,8'b0})
            $fatal(1,"bad bounded-request UR");
    endtask
    integer previous_count;
    time started;
    initial begin
        #1; for(integer j=0;j<16384;j++) dut.rom[j]=32'ha5000000+j;
        reset_transaction();
        repeat(100) @(negedge clk);
        if(out_valid || flash.transactions!=0) $fatal(1,"unsolicited request or completion");
        read_request(0,1,8'h0f,0,8'h02); completion(0,1,4,0,2,0);
        read_request('hffffc,1,8'h0f,1,8'h02); completion('hffffc,1,4,'h7c,2,0);
        // Entire 8-DWORD request crosses a completion boundary, within one page.
        started=$time;
        read_request('habc3c,8,8'hff,1,8'h02);
        completion('habc3c,1,32,'h3c,2,0);
        completion('habc40,7,28,'h40,2,0);
        if($time-started>40000) $fatal(1,"payload request exceeds 40us nominal service");
        // Requester zero is valid: it must be echoed exactly, never invented.
        requester=0; request_tag=0;
        read_request('h22000,1,8'h00,0,8'h02); completion('h22000,1,1,0,2,0);
        requester=16'h1234; request_tag=8'h56;
        // Enlarged ROM upper pages cannot alias the original 8KiB aperture.
        read_request('hfffc,1,8'h0f,0,8'h40); completion('hfffc,1,4,'h7c,1,0);
        read_request('h803c,16,8'hff,0,8'h40);
        completion('h803c,1,64,'h3c,1,0); completion('h8040,15,60,'h40,1,0);
        previous_count=flash.transactions;
        beat({32'h1234560f,32'h40000001},8'hff,0,8'h02);
        beat({32'h06000000,32'h00000000},8'hff,1,8'h02);
        wait(!busy); repeat(10) @(negedge clk);
        if(out_valid || flash.transactions!=previous_count) $fatal(1,"BAR1 write touched flash/TX");
        read_request(0,9,8'hff,0,8'h02); unsupported();
        if(flash.transactions!=previous_count) $fatal(1,"oversized read touched flash");
        read_request('hffffc,2,8'hff,0,8'h02);
        wait(!busy); repeat(10) @(negedge clk);
        if(out_valid || flash.transactions!=previous_count || !rx_fault) $fatal(1,"out-of-slot read");
        reset_transaction();
        // Absent flash has finite all-ones data; software envelope validation
        // rejects it without requiring an electrical presence signal.
        flash.absent=1;
        read_request(0,1,8'h0f,0,8'h02); completion(0,1,4,0,0,32'hffffffff);
        flash.absent=0;
        // Lost transport response is cancelled and erased for the remaining
        // request words, with a persistent RX diagnostic and valid guarded TX.
        force payload_response_valid=0;
        previous_count=flash.transactions; started=$time;
        read_request(0,8,8'hff,0,8'h02); completion(0,8,32,0,0,32'hffffffff);
        release payload_response_valid;
        if(!rx_fault || flash.transactions!=previous_count+1 || $time-started>9000)
            $fatal(1,"timeout was not bounded/fail-closed");
        reset_transaction();
        force payload_response_error=1;
        read_request(0,2,8'hff,0,8'h02); completion(0,2,8,0,0,32'hffffffff);
        release payload_response_error;
        if(!rx_fault) $fatal(1,"missing transport error diagnostic");
        reset_transaction();
        read_request(0,8,8'hff,0,8'h02);
        wait(!spi_cs_n); repeat(80) @(negedge clk);
        reset_transaction();
        if(!spi_cs_n || out_valid) $fatal(1,"reset failed to abort SPI/TX");
        read_request('h34560,1,8'h0f,0,8'h02); completion('h34560,1,4,'h60,2,0);
        if(fault) $fatal(1,"guard failed");
        $display("PASS: payload BAR1 SPI, bounded latency, requester identity, large ROM, writes, UR, bounds, absence, timeout and reset");
        $finish;
    end
    initial begin #1000000; $fatal(1,"timeout"); end
endmodule
