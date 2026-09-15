`timescale 1ns/1ps
// Independent behavioral IS25LP256D 1-1-1 normal-read model. It accepts only
// 13h at the declared slot, samples SI on rising edges, launches SO on falling
// edges with the datasheet's worst-case 8ns output-valid delay.
module svmvisor_spi_flash_model (
    input wire spi_clk, spi_cs_n, spi_mosi,
    output reg spi_miso=1
);
    reg [39:0] captured=0;
    integer bits=0, transactions=0;
    reg [31:0] address=0;
    reg absent=0;
    time last_mosi_change=0, last_clock_rise=0, last_clock_edge=0, last_select=0;
    always @(spi_mosi) begin
        if(!spi_cs_n && last_clock_rise!=0 && $time-last_clock_rise<2)
            $fatal(1,"flash SI hold violated");
        last_mosi_change=$time;
    end
    function automatic [7:0] byte_at(input logic [31:0] a);
        return a[7:0] ^ a[15:8] ^ a[23:16] ^ 8'h5a;
    endfunction
    function automatic [31:0] word_at(input logic [31:0] a);
        return {byte_at(a+3),byte_at(a+2),byte_at(a+1),byte_at(a)};
    endfunction
    always @(negedge spi_cs_n) begin last_select=$time; bits=0; captured=0; spi_miso=1; transactions++; end
    always @(posedge spi_clk) if(!spi_cs_n) begin
        if(bits==0 && $time-last_select<3) $fatal(1,"flash CE setup violated");
        if($time-last_mosi_change<2) $fatal(1,"flash SI setup violated");
        last_clock_rise=$time;
        if(bits<40) captured={captured[38:0],spi_mosi};
        bits++;
        if(bits==40) begin
            if(captured[39:32] !== 8'h13 || captured[31:20] !== 12'h004 || captured[1:0] !== 0)
                $fatal(1,"illegal flash command/address %h",captured);
            address=captured[31:0];
        end
    end
    always @(negedge spi_clk) if(!spi_cs_n && bits>=40 && bits<72)
        spi_miso <= #8 (absent ? 1'b1 : (byte_at(address+(bits-40)/8) >> (7-(bits-40)%8)) & 1'b1);
    always @(spi_clk) if(!spi_cs_n) last_clock_edge=$time;
    always @(posedge spi_cs_n) begin
        // Reset/cancel may asynchronously terminate an incomplete read.
        // Completed reads must respect the last delayed clock edge's CE hold.
        if(bits==72 && $time-last_clock_edge<3) $fatal(1,"flash CE hold violated");
        spi_miso <= 1;
    end
endmodule
