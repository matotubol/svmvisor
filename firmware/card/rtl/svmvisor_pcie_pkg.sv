package svmvisor_pcie_pkg;
    // PG054: 64-bit AXI stream puts header DWORD 0 in bits 31:0.
    typedef struct packed {
        logic [15:0] completer;
        logic [15:0] requester;
        logic [7:0] tag;
        logic [2:0] tc;
        logic [2:0] attr;
        logic [11:0] address;
        logic [10:0] words;
        logic [12:0] bytes_left;
        logic [1:0] first_offset;
        logic ur;
    } read_token_t;

    function automatic [1:0] first_byte(input logic [3:0] be);
        if (be[0]) return 0;
        if (be[1]) return 1;
        if (be[2]) return 2;
        if (be[3]) return 3;
        return 0;
    endfunction
    function automatic [1:0] last_byte(input logic [3:0] be);
        if (be[3]) return 3;
        if (be[2]) return 2;
        if (be[1]) return 1;
        return 0;
    endfunction
    function automatic [31:0] swap_bytes(input logic [31:0] d);
        return {d[7:0], d[15:8], d[23:16], d[31:24]};
    endfunction
    function automatic [4:0] chunk_words(input read_token_t t);
        logic [4:0] boundary;
        boundary = 5'd16 - {1'b0, t.address[5:2]};
        return t.words < boundary ? t.words[4:0] : boundary;
    endfunction
    function automatic [31:0] completion_h0(input read_token_t t);
        return (t.ur ? 32'h0a000000 : 32'h4a000000) |
            ({29'b0,t.tc} << 20) | ({31'b0,t.attr[2]} << 18) |
            ({30'b0,t.attr[1:0]} << 12) | (t.ur ? 32'd0 : {27'b0,chunk_words(t)});
    endfunction
    function automatic [31:0] completion_h1(input read_token_t t);
        return {t.completer, (t.ur ? 3'b001 : 3'b000), 1'b0,
            (t.ur ? 12'b0 : t.bytes_left[11:0])};
    endfunction
    function automatic [31:0] completion_h2(input read_token_t t);
        return {t.requester,t.tag,1'b0,(t.ur ? 7'b0 : {t.address[6:2],t.first_offset})};
    endfunction
    function automatic read_token_t advance_token(input read_token_t t);
        read_token_t n;
        logic [4:0] words;
        words = chunk_words(t);
        n = t;
        n.address = t.address + {5'b0,words,2'b0};
        n.words = t.words - words;
        n.bytes_left = t.bytes_left - ({6'b0,words,2'b0} - t.first_offset);
        n.first_offset = 0;
        return n;
    endfunction
endpackage
