// ==============================================================
// Vitis HLS - High-Level Synthesis from C, C++ and OpenCL v2025.2 (64-bit)
// Tool Version Limit: 2025.11
// Copyright 1986-2022 Xilinx, Inc. All Rights Reserved.
// Copyright 2022-2025 Advanced Micro Devices, Inc. All Rights Reserved.
//
// ==============================================================
`timescale 1ns/1ps
(* DowngradeIPIdentifiedWarnings="yes" *) module Scalars_control_s_axi
#(parameter
    C_S_AXI_ADDR_WIDTH = 7,
    C_S_AXI_DATA_WIDTH = 32
)(
    input  wire                          ACLK,
    input  wire                          ARESET,
    input  wire                          ACLK_EN,
    input  wire [C_S_AXI_ADDR_WIDTH-1:0] AWADDR,
    input  wire                          AWVALID,
    output wire                          AWREADY,
    input  wire [C_S_AXI_DATA_WIDTH-1:0] WDATA,
    input  wire [C_S_AXI_DATA_WIDTH/8-1:0] WSTRB,
    input  wire                          WVALID,
    output wire                          WREADY,
    output wire [1:0]                    BRESP,
    output wire                          BVALID,
    input  wire                          BREADY,
    input  wire [C_S_AXI_ADDR_WIDTH-1:0] ARADDR,
    input  wire                          ARVALID,
    output wire                          ARREADY,
    output wire [C_S_AXI_DATA_WIDTH-1:0] RDATA,
    output wire [1:0]                    RRESP,
    output wire                          RVALID,
    input  wire                          RREADY,
    output wire                          interrupt,
    output wire [63:0]                   result_offset,
    output wire [79:0]                   a,
    output wire [31:0]                   b,
    output wire [11:0]                   c,
    output wire [63:0]                   d,
    output wire [11:0]                   e,
    output wire [79:0]                   f,
    output wire                          ap_start,
    input  wire                          ap_done,
    input  wire                          ap_ready,
    input  wire                          ap_idle
);
//------------------------Address Info-------------------
// Protocol Used: ap_ctrl_hs
//
// 0x00 : Control signals
//        bit 0  - ap_start (Read/Write/SC)
//        bit 1  - ap_done (Read/COR)
//        bit 2  - ap_idle (Read)
//        bit 3  - ap_ready (Read/COR)
//        bit 7  - auto_restart (Read/Write)
//        bit 9  - interrupt (Read)
//        others - reserved
// 0x04 : Global Interrupt Enable Register
//        bit 0  - Global Interrupt Enable (Read/Write)
//        others - reserved
// 0x08 : IP Interrupt Enable Register (Read/Write)
//        bit 0 - enable ap_done interrupt (Read/Write)
//        others - reserved
// 0x0c : IP Interrupt Status Register (Read/TOW)
//        bit 0 - ap_done (Read/TOW)
//        others - reserved
// 0x10 : Data signal of result_offset
//        bit 31~0 - result_offset[31:0] (Read/Write)
// 0x14 : Data signal of result_offset
//        bit 31~0 - result_offset[63:32] (Read/Write)
// 0x18 : reserved
// 0x1c : Data signal of a
//        bit 31~0 - a[31:0] (Read/Write)
// 0x20 : Data signal of a
//        bit 31~0 - a[63:32] (Read/Write)
// 0x24 : Data signal of a
//        bit 15~0 - a[79:64] (Read/Write)
//        others   - reserved
// 0x28 : reserved
// 0x2c : Data signal of b
//        bit 31~0 - b[31:0] (Read/Write)
// 0x30 : reserved
// 0x34 : Data signal of c
//        bit 11~0 - c[11:0] (Read/Write)
//        others   - reserved
// 0x38 : reserved
// 0x3c : Data signal of d
//        bit 31~0 - d[31:0] (Read/Write)
// 0x40 : Data signal of d
//        bit 31~0 - d[63:32] (Read/Write)
// 0x44 : reserved
// 0x48 : Data signal of e
//        bit 11~0 - e[11:0] (Read/Write)
//        others   - reserved
// 0x4c : reserved
// 0x50 : Data signal of f
//        bit 31~0 - f[31:0] (Read/Write)
// 0x54 : Data signal of f
//        bit 31~0 - f[63:32] (Read/Write)
// 0x58 : Data signal of f
//        bit 15~0 - f[79:64] (Read/Write)
//        others   - reserved
// 0x5c : reserved
// (SC = Self Clear, COR = Clear on Read, TOW = Toggle on Write, COH = Clear on Handshake)

//------------------------Parameter----------------------
localparam
    ADDR_AP_CTRL              = 7'h00,
    ADDR_GIE                  = 7'h04,
    ADDR_IER                  = 7'h08,
    ADDR_ISR                  = 7'h0c,
    ADDR_RESULT_OFFSET_DATA_0 = 7'h10,
    ADDR_RESULT_OFFSET_DATA_1 = 7'h14,
    ADDR_RESULT_OFFSET_CTRL   = 7'h18,
    ADDR_A_DATA_0             = 7'h1c,
    ADDR_A_DATA_1             = 7'h20,
    ADDR_A_DATA_2             = 7'h24,
    ADDR_A_CTRL               = 7'h28,
    ADDR_B_DATA_0             = 7'h2c,
    ADDR_B_CTRL               = 7'h30,
    ADDR_C_DATA_0             = 7'h34,
    ADDR_C_CTRL               = 7'h38,
    ADDR_D_DATA_0             = 7'h3c,
    ADDR_D_DATA_1             = 7'h40,
    ADDR_D_CTRL               = 7'h44,
    ADDR_E_DATA_0             = 7'h48,
    ADDR_E_CTRL               = 7'h4c,
    ADDR_F_DATA_0             = 7'h50,
    ADDR_F_DATA_1             = 7'h54,
    ADDR_F_DATA_2             = 7'h58,
    ADDR_F_CTRL               = 7'h5c,
    WRIDLE                    = 2'd0,
    WRDATA                    = 2'd1,
    WRRESP                    = 2'd2,
    WRRESET                   = 2'd3,
    RDIDLE                    = 2'd0,
    RDDATA                    = 2'd1,
    RDRESET                   = 2'd2,
    ADDR_BITS                = 7;

//------------------------Local signal-------------------
    reg  [1:0]                    wstate = WRRESET;
    reg  [1:0]                    wnext;
    reg  [ADDR_BITS-1:0]          waddr;
    wire [C_S_AXI_DATA_WIDTH-1:0] wmask;
    wire                          aw_hs;
    wire                          w_hs;
    reg  [1:0]                    rstate = RDRESET;
    reg  [1:0]                    rnext;
    reg  [C_S_AXI_DATA_WIDTH-1:0] rdata;
    wire                          ar_hs;
    wire [ADDR_BITS-1:0]          raddr;
    // internal registers
    reg                           int_ap_idle = 1'b0;
    reg                           int_ap_ready = 1'b0;
    wire                          task_ap_ready;
    reg                           int_ap_done = 1'b0;
    wire                          task_ap_done;
    reg                           int_task_ap_done = 1'b0;
    reg                           int_ap_start = 1'b0;
    reg                           int_interrupt = 1'b0;
    reg                           int_auto_restart = 1'b0;
    reg                           auto_restart_status = 1'b0;
    wire                          auto_restart_done;
    reg                           int_gie = 1'b0;
    reg                           int_ier = 1'b0;
    reg                           int_isr = 1'b0;
    reg  [63:0]                   int_result_offset = 'b0;
    reg  [79:0]                   int_a = 'b0;
    reg  [31:0]                   int_b = 'b0;
    reg  [11:0]                   int_c = 'b0;
    reg  [63:0]                   int_d = 'b0;
    reg  [11:0]                   int_e = 'b0;
    reg  [79:0]                   int_f = 'b0;

//------------------------Instantiation------------------


//------------------------AXI write fsm------------------
assign AWREADY = (wstate == WRIDLE);
assign WREADY  = (wstate == WRDATA);
assign BVALID  = (wstate == WRRESP);
assign BRESP   = 2'b00;  // OKAY
assign wmask   = { {8{WSTRB[3]}}, {8{WSTRB[2]}}, {8{WSTRB[1]}}, {8{WSTRB[0]}} };
assign aw_hs   = AWVALID & AWREADY;
assign w_hs    = WVALID & WREADY;

// wstate
always @(posedge ACLK) begin
    if (ARESET)
        wstate <= WRRESET;
    else if (ACLK_EN)
        wstate <= wnext;
end

// wnext
always @(*) begin
    case (wstate)
        WRIDLE:
            if (AWVALID)
                wnext = WRDATA;
            else
                wnext = WRIDLE;
        WRDATA:
            if (WVALID)
                wnext = WRRESP;
            else
                wnext = WRDATA;
        WRRESP:
            if (BREADY & BVALID)
                wnext = WRIDLE;
            else
                wnext = WRRESP;
        default:
            wnext = WRIDLE;
    endcase
end

// waddr
always @(posedge ACLK) begin
    if (ACLK_EN) begin
        if (aw_hs)
            waddr <= {AWADDR[ADDR_BITS-1:2], {2{1'b0}}};
    end
end

//------------------------AXI read fsm-------------------
assign ARREADY = (rstate == RDIDLE);
assign RDATA   = rdata;
assign RRESP   = 2'b00;  // OKAY
assign RVALID  = (rstate == RDDATA);
assign ar_hs   = ARVALID & ARREADY;
assign raddr   = ARADDR[ADDR_BITS-1:0];

// rstate
always @(posedge ACLK) begin
    if (ARESET)
        rstate <= RDRESET;
    else if (ACLK_EN)
        rstate <= rnext;
end

// rnext
always @(*) begin
    case (rstate)
        RDIDLE:
            if (ARVALID)
                rnext = RDDATA;
            else
                rnext = RDIDLE;
        RDDATA:
            if (RREADY & RVALID)
                rnext = RDIDLE;
            else
                rnext = RDDATA;
        default:
            rnext = RDIDLE;
    endcase
end

// rdata
always @(posedge ACLK) begin
    if (ACLK_EN) begin
        if (ar_hs) begin
            rdata <= 'b0;
            case (raddr)
                ADDR_AP_CTRL: begin
                    rdata[0] <= int_ap_start;
                    rdata[1] <= int_task_ap_done;
                    rdata[2] <= int_ap_idle;
                    rdata[3] <= int_ap_ready;
                    rdata[7] <= int_auto_restart;
                    rdata[9] <= int_interrupt;
                end
                ADDR_GIE: begin
                    rdata <= int_gie;
                end
                ADDR_IER: begin
                    rdata <= int_ier;
                end
                ADDR_ISR: begin
                    rdata <= int_isr;
                end
                ADDR_RESULT_OFFSET_DATA_0: begin
                    rdata <= int_result_offset[31:0];
                end
                ADDR_RESULT_OFFSET_DATA_1: begin
                    rdata <= int_result_offset[63:32];
                end
                ADDR_A_DATA_0: begin
                    rdata <= int_a[31:0];
                end
                ADDR_A_DATA_1: begin
                    rdata <= int_a[63:32];
                end
                ADDR_A_DATA_2: begin
                    rdata <= int_a[79:64];
                end
                ADDR_B_DATA_0: begin
                    rdata <= int_b[31:0];
                end
                ADDR_C_DATA_0: begin
                    rdata <= int_c[11:0];
                end
                ADDR_D_DATA_0: begin
                    rdata <= int_d[31:0];
                end
                ADDR_D_DATA_1: begin
                    rdata <= int_d[63:32];
                end
                ADDR_E_DATA_0: begin
                    rdata <= int_e[11:0];
                end
                ADDR_F_DATA_0: begin
                    rdata <= int_f[31:0];
                end
                ADDR_F_DATA_1: begin
                    rdata <= int_f[63:32];
                end
                ADDR_F_DATA_2: begin
                    rdata <= int_f[79:64];
                end
            endcase
        end
    end
end


//------------------------Register logic-----------------
assign interrupt         = int_interrupt;
assign ap_start          = int_ap_start;
assign task_ap_done      = (ap_done && !auto_restart_status) || auto_restart_done;
assign task_ap_ready     = ap_ready && !int_auto_restart;
assign auto_restart_done = auto_restart_status && (ap_idle && !int_ap_idle);
assign result_offset     = int_result_offset;
assign a                 = int_a;
assign b                 = int_b;
assign c                 = int_c;
assign d                 = int_d;
assign e                 = int_e;
assign f                 = int_f;
// int_interrupt
always @(posedge ACLK) begin
    if (ARESET)
        int_interrupt <= 1'b0;
    else if (ACLK_EN) begin
        if (int_gie && (|int_isr))
            int_interrupt <= 1'b1;
        else
            int_interrupt <= 1'b0;
    end
end

// int_ap_start
always @(posedge ACLK) begin
    if (ARESET)
        int_ap_start <= 1'b0;
    else if (ACLK_EN) begin
        if (w_hs && waddr == ADDR_AP_CTRL && WSTRB[0] && WDATA[0])
            int_ap_start <= 1'b1;
        else if (ap_done & int_auto_restart)
            int_ap_start <= 1'b1; // auto restart
        else
            int_ap_start <= 1'b0; // self clear
    end
end

// int_ap_done
always @(posedge ACLK) begin
    if (ARESET)
        int_ap_done <= 1'b0;
    else if (ACLK_EN) begin
            int_ap_done <= ap_done;
    end
end

// int_task_ap_done
always @(posedge ACLK) begin
    if (ARESET)
        int_task_ap_done <= 1'b0;
    else if (ACLK_EN) begin
        if (task_ap_done)
            int_task_ap_done <= 1'b1;
        else if (ar_hs && raddr == ADDR_AP_CTRL)
            int_task_ap_done <= 1'b0; // clear on read
    end
end

// int_ap_idle
always @(posedge ACLK) begin
    if (ARESET)
        int_ap_idle <= 1'b0;
    else if (ACLK_EN) begin
            int_ap_idle <= ap_idle;
    end
end

// int_ap_ready
always @(posedge ACLK) begin
    if (ARESET)
        int_ap_ready <= 1'b0;
    else if (ACLK_EN) begin
        if (task_ap_ready)
            int_ap_ready <= 1'b1;
        else if (ar_hs && raddr == ADDR_AP_CTRL)
            int_ap_ready <= 1'b0;
    end
end

// int_auto_restart
always @(posedge ACLK) begin
    if (ARESET)
        int_auto_restart <= 1'b0;
    else if (ACLK_EN) begin
        if (w_hs && waddr == ADDR_AP_CTRL && WSTRB[0])
            int_auto_restart <= WDATA[7];
    end
end

// auto_restart_status
always @(posedge ACLK) begin
    if (ARESET)
        auto_restart_status <= 1'b0;
    else if (ACLK_EN) begin
        if (int_auto_restart)
            auto_restart_status <= 1'b1;
        else if (ap_idle)
            auto_restart_status <= 1'b0;
    end
end

// int_gie
always @(posedge ACLK) begin
    if (ARESET)
        int_gie <= 1'b0;
    else if (ACLK_EN) begin
        if (w_hs && waddr == ADDR_GIE && WSTRB[0])
            int_gie <= WDATA[0];
    end
end

// int_ier
always @(posedge ACLK) begin
    if (ARESET)
        int_ier <= 1'b0;
    else if (ACLK_EN) begin
        if (w_hs && waddr == ADDR_IER && WSTRB[0])
            int_ier <= WDATA[0];
    end
end

// int_isr
always @(posedge ACLK) begin
    if (ARESET)
        int_isr <= 1'b0;
    else if (ACLK_EN) begin
        if (int_ier & ap_done)
            int_isr <= 1'b1;
        else if (w_hs && waddr == ADDR_ISR && WSTRB[0])
            int_isr <= int_isr ^ WDATA[0]; // toggle on write
    end
end

// int_result_offset[31:0]
always @(posedge ACLK) begin
    if (ARESET)
        int_result_offset[31:0] <= 0;
    else if (ACLK_EN) begin
        if (w_hs && waddr == ADDR_RESULT_OFFSET_DATA_0)
            int_result_offset[31:0] <= (WDATA[31:0] & wmask) | (int_result_offset[31:0] & ~wmask);
    end
end

// int_result_offset[63:32]
always @(posedge ACLK) begin
    if (ARESET)
        int_result_offset[63:32] <= 0;
    else if (ACLK_EN) begin
        if (w_hs && waddr == ADDR_RESULT_OFFSET_DATA_1)
            int_result_offset[63:32] <= (WDATA[31:0] & wmask) | (int_result_offset[63:32] & ~wmask);
    end
end

// int_a[31:0]
always @(posedge ACLK) begin
    if (ARESET)
        int_a[31:0] <= 0;
    else if (ACLK_EN) begin
        if (w_hs && waddr == ADDR_A_DATA_0)
            int_a[31:0] <= (WDATA[31:0] & wmask) | (int_a[31:0] & ~wmask);
    end
end

// int_a[63:32]
always @(posedge ACLK) begin
    if (ARESET)
        int_a[63:32] <= 0;
    else if (ACLK_EN) begin
        if (w_hs && waddr == ADDR_A_DATA_1)
            int_a[63:32] <= (WDATA[31:0] & wmask) | (int_a[63:32] & ~wmask);
    end
end

// int_a[79:64]
always @(posedge ACLK) begin
    if (ARESET)
        int_a[79:64] <= 0;
    else if (ACLK_EN) begin
        if (w_hs && waddr == ADDR_A_DATA_2)
            int_a[79:64] <= (WDATA[31:0] & wmask) | (int_a[79:64] & ~wmask);
    end
end

// int_b[31:0]
always @(posedge ACLK) begin
    if (ARESET)
        int_b[31:0] <= 0;
    else if (ACLK_EN) begin
        if (w_hs && waddr == ADDR_B_DATA_0)
            int_b[31:0] <= (WDATA[31:0] & wmask) | (int_b[31:0] & ~wmask);
    end
end

// int_c[11:0]
always @(posedge ACLK) begin
    if (ARESET)
        int_c[11:0] <= 0;
    else if (ACLK_EN) begin
        if (w_hs && waddr == ADDR_C_DATA_0)
            int_c[11:0] <= (WDATA[31:0] & wmask) | (int_c[11:0] & ~wmask);
    end
end

// int_d[31:0]
always @(posedge ACLK) begin
    if (ARESET)
        int_d[31:0] <= 0;
    else if (ACLK_EN) begin
        if (w_hs && waddr == ADDR_D_DATA_0)
            int_d[31:0] <= (WDATA[31:0] & wmask) | (int_d[31:0] & ~wmask);
    end
end

// int_d[63:32]
always @(posedge ACLK) begin
    if (ARESET)
        int_d[63:32] <= 0;
    else if (ACLK_EN) begin
        if (w_hs && waddr == ADDR_D_DATA_1)
            int_d[63:32] <= (WDATA[31:0] & wmask) | (int_d[63:32] & ~wmask);
    end
end

// int_e[11:0]
always @(posedge ACLK) begin
    if (ARESET)
        int_e[11:0] <= 0;
    else if (ACLK_EN) begin
        if (w_hs && waddr == ADDR_E_DATA_0)
            int_e[11:0] <= (WDATA[31:0] & wmask) | (int_e[11:0] & ~wmask);
    end
end

// int_f[31:0]
always @(posedge ACLK) begin
    if (ARESET)
        int_f[31:0] <= 0;
    else if (ACLK_EN) begin
        if (w_hs && waddr == ADDR_F_DATA_0)
            int_f[31:0] <= (WDATA[31:0] & wmask) | (int_f[31:0] & ~wmask);
    end
end

// int_f[63:32]
always @(posedge ACLK) begin
    if (ARESET)
        int_f[63:32] <= 0;
    else if (ACLK_EN) begin
        if (w_hs && waddr == ADDR_F_DATA_1)
            int_f[63:32] <= (WDATA[31:0] & wmask) | (int_f[63:32] & ~wmask);
    end
end

// int_f[79:64]
always @(posedge ACLK) begin
    if (ARESET)
        int_f[79:64] <= 0;
    else if (ACLK_EN) begin
        if (w_hs && waddr == ADDR_F_DATA_2)
            int_f[79:64] <= (WDATA[31:0] & wmask) | (int_f[79:64] & ~wmask);
    end
end

//synthesis translate_off
always @(posedge ACLK) begin
    if (ACLK_EN) begin
        if (int_gie & ~int_isr & int_ier & ap_done)
            $display ("// Interrupt Monitor : interrupt for ap_done detected @ \"%0t\"", $time);
    end
end
//synthesis translate_on

//------------------------Memory logic-------------------

endmodule
