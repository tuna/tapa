module Stream2Mmap #(
  parameter ap_ST_fsm_state1 = 8'd1,
  parameter ap_ST_fsm_state2 = 8'd2,
  parameter ap_ST_fsm_state3 = 8'd4,
  parameter ap_ST_fsm_state4 = 8'd8,
  parameter ap_ST_fsm_state5 = 8'd16,
  parameter ap_ST_fsm_state6 = 8'd32,
  parameter ap_ST_fsm_state7 = 8'd64,
  parameter ap_ST_fsm_state8 = 8'd128,
  parameter C_M_AXI_MMAP_ID_WIDTH = 1,
  parameter C_M_AXI_MMAP_ADDR_WIDTH = 64,
  parameter C_M_AXI_MMAP_DATA_WIDTH = 32,
  parameter C_M_AXI_MMAP_AWUSER_WIDTH = 1,
  parameter C_M_AXI_MMAP_ARUSER_WIDTH = 1,
  parameter C_M_AXI_MMAP_WUSER_WIDTH = 1,
  parameter C_M_AXI_MMAP_RUSER_WIDTH = 1,
  parameter C_M_AXI_MMAP_BUSER_WIDTH = 1,
  parameter C_M_AXI_MMAP_USER_VALUE = 0,
  parameter C_M_AXI_MMAP_PROT_VALUE = 0,
  parameter C_M_AXI_MMAP_CACHE_VALUE = 3,
  parameter C_M_AXI_DATA_WIDTH = 32,
  parameter C_M_AXI_MMAP_WSTRB_WIDTH = (32/8),
  parameter C_M_AXI_WSTRB_WIDTH = (32/8)
) (
  input wire ap_clk,
  input wire ap_rst_n,
  input wire ap_start,
  output wire ap_done,
  output wire ap_idle,
  output wire ap_ready,
  output wire m_axi_mmap_AWVALID,
  input wire m_axi_mmap_AWREADY,
  output wire [C_M_AXI_MMAP_ADDR_WIDTH-1:0] m_axi_mmap_AWADDR,
  output wire [C_M_AXI_MMAP_ID_WIDTH-1:0] m_axi_mmap_AWID,
  output wire [7:0] m_axi_mmap_AWLEN,
  output wire [2:0] m_axi_mmap_AWSIZE,
  output wire [1:0] m_axi_mmap_AWBURST,
  output wire [1:0] m_axi_mmap_AWLOCK,
  output wire [3:0] m_axi_mmap_AWCACHE,
  output wire [2:0] m_axi_mmap_AWPROT,
  output wire [3:0] m_axi_mmap_AWQOS,
  output wire [3:0] m_axi_mmap_AWREGION,
  output wire [C_M_AXI_MMAP_AWUSER_WIDTH-1:0] m_axi_mmap_AWUSER,
  output wire m_axi_mmap_WVALID,
  input wire m_axi_mmap_WREADY,
  output wire [C_M_AXI_MMAP_DATA_WIDTH-1:0] m_axi_mmap_WDATA,
  output wire [C_M_AXI_MMAP_WSTRB_WIDTH-1:0] m_axi_mmap_WSTRB,
  output wire m_axi_mmap_WLAST,
  output wire [C_M_AXI_MMAP_ID_WIDTH-1:0] m_axi_mmap_WID,
  output wire [C_M_AXI_MMAP_WUSER_WIDTH-1:0] m_axi_mmap_WUSER,
  output wire m_axi_mmap_ARVALID,
  input wire m_axi_mmap_ARREADY,
  output wire [C_M_AXI_MMAP_ADDR_WIDTH-1:0] m_axi_mmap_ARADDR,
  output wire [C_M_AXI_MMAP_ID_WIDTH-1:0] m_axi_mmap_ARID,
  output wire [7:0] m_axi_mmap_ARLEN,
  output wire [2:0] m_axi_mmap_ARSIZE,
  output wire [1:0] m_axi_mmap_ARBURST,
  output wire [1:0] m_axi_mmap_ARLOCK,
  output wire [3:0] m_axi_mmap_ARCACHE,
  output wire [2:0] m_axi_mmap_ARPROT,
  output wire [3:0] m_axi_mmap_ARQOS,
  output wire [3:0] m_axi_mmap_ARREGION,
  output wire [C_M_AXI_MMAP_ARUSER_WIDTH-1:0] m_axi_mmap_ARUSER,
  input wire m_axi_mmap_RVALID,
  output wire m_axi_mmap_RREADY,
  input wire [C_M_AXI_MMAP_DATA_WIDTH-1:0] m_axi_mmap_RDATA,
  input wire m_axi_mmap_RLAST,
  input wire [C_M_AXI_MMAP_ID_WIDTH-1:0] m_axi_mmap_RID,
  input wire [C_M_AXI_MMAP_RUSER_WIDTH-1:0] m_axi_mmap_RUSER,
  input wire [1:0] m_axi_mmap_RRESP,
  input wire m_axi_mmap_BVALID,
  output wire m_axi_mmap_BREADY,
  input wire [1:0] m_axi_mmap_BRESP,
  input wire [C_M_AXI_MMAP_ID_WIDTH-1:0] m_axi_mmap_BID,
  input wire [C_M_AXI_MMAP_BUSER_WIDTH-1:0] m_axi_mmap_BUSER,
  input wire [32:0] stream_s_dout,
  input wire stream_s_empty_n,
  output wire stream_s_read,
  input wire [32:0] stream_peek_dout,
  input wire stream_peek_empty_n,
  output wire stream_peek_read,
  input wire [63:0] mmap_offset,
  input wire [63:0] n
);

reg ap_done;
reg ap_idle;
reg ap_ready;
reg stream_s_read;
reg ap_rst_n_inv;
reg [7:0] ap_CS_fsm;
wire ap_CS_fsm_state1;
reg mmap_blk_n_AW;
reg mmap_blk_n_B;
wire ap_CS_fsm_state8;
reg [63:0] n_read_reg_123;
wire [61:0] trunc_ln_fu_102_p4;
reg [61:0] trunc_ln_reg_128;
wire grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_ap_start;
wire grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_ap_done;
wire grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_ap_idle;
wire grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_ap_ready;
wire grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_stream_s_read;
wire grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_AWVALID;
wire [63:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_AWADDR;
wire [0:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_AWID;
wire [31:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_AWLEN;
wire [2:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_AWSIZE;
wire [1:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_AWBURST;
wire [1:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_AWLOCK;
wire [3:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_AWCACHE;
wire [2:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_AWPROT;
wire [3:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_AWQOS;
wire [3:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_AWREGION;
wire [0:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_AWUSER;
wire grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_WVALID;
wire [31:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_WDATA;
wire [3:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_WSTRB;
wire grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_WLAST;
wire [0:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_WID;
wire [0:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_WUSER;
wire grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_ARVALID;
wire [63:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_ARADDR;
wire [0:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_ARID;
wire [31:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_ARLEN;
wire [2:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_ARSIZE;
wire [1:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_ARBURST;
wire [1:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_ARLOCK;
wire [3:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_ARCACHE;
wire [2:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_ARPROT;
wire [3:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_ARQOS;
wire [3:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_ARREGION;
wire [0:0] grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_ARUSER;
wire grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_RREADY;
wire grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_BREADY;
reg mmap_0_AWVALID;
wire mmap_0_AWREADY;
reg [63:0] mmap_0_AWADDR;
reg [31:0] mmap_0_AWLEN;
reg mmap_0_WVALID;
wire mmap_0_WREADY;
wire mmap_0_ARREADY;
wire mmap_0_RVALID;
wire [31:0] mmap_0_RDATA;
wire [8:0] mmap_0_RFIFONUM;
wire mmap_0_BVALID;
reg mmap_0_BREADY;
reg grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_ap_start_reg;
reg [7:0] ap_NS_fsm;
wire ap_NS_fsm_state2;
wire ap_CS_fsm_state2;
wire ap_CS_fsm_state3;
wire [63:0] sext_ln32_fu_112_p1;
reg ap_ST_fsm_state1_blk;
wire ap_ST_fsm_state2_blk;
reg ap_ST_fsm_state3_blk;
wire ap_ST_fsm_state4_blk;
wire ap_ST_fsm_state5_blk;
wire ap_ST_fsm_state6_blk;
wire ap_ST_fsm_state7_blk;
reg ap_ST_fsm_state8_blk;
wire ap_ce_reg;

always @ (posedge ap_clk) begin
    if (ap_rst_n_inv == 1'b1) begin
        ap_CS_fsm <= ap_ST_fsm_state1;
    end else begin
        ap_CS_fsm <= ap_NS_fsm;
    end
end

always @ (posedge ap_clk) begin
    if (ap_rst_n_inv == 1'b1) begin
        grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_ap_start_reg <= 1'b0;
    end else begin
        if (((1'b1 == ap_NS_fsm_state2) & (1'b1 == ap_CS_fsm_state1))) begin
            grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_ap_start_reg <= 1'b1;
        end else if ((grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_ap_ready == 1'b1)) begin
            grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_ap_start_reg <= 1'b0;
        end
    end
end

always @ (posedge ap_clk) begin
    if ((1'b1 == ap_CS_fsm_state1)) begin
        n_read_reg_123 <= n;
        trunc_ln_reg_128 <= {{mmap_offset[63:2]}};
    end
end

always @ (*) begin
    if (((mmap_0_AWREADY == 1'b0) | (ap_start == 1'b0))) begin
        ap_ST_fsm_state1_blk = 1'b1;
    end else begin
        ap_ST_fsm_state1_blk = 1'b0;
    end
end

assign ap_ST_fsm_state2_blk = 1'b0;

always @ (*) begin
    if ((grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_ap_done == 1'b0)) begin
        ap_ST_fsm_state3_blk = 1'b1;
    end else begin
        ap_ST_fsm_state3_blk = 1'b0;
    end
end

assign ap_ST_fsm_state4_blk = 1'b0;

assign ap_ST_fsm_state5_blk = 1'b0;

assign ap_ST_fsm_state6_blk = 1'b0;

assign ap_ST_fsm_state7_blk = 1'b0;

always @ (*) begin
    if ((mmap_0_BVALID == 1'b0)) begin
        ap_ST_fsm_state8_blk = 1'b1;
    end else begin
        ap_ST_fsm_state8_blk = 1'b0;
    end
end

always @ (*) begin
    if (((mmap_0_BVALID == 1'b1) & (1'b1 == ap_CS_fsm_state8))) begin
        ap_done = 1'b1;
    end else begin
        ap_done = 1'b0;
    end
end

always @ (*) begin
    if (((1'b1 == ap_CS_fsm_state1) & (ap_start == 1'b0))) begin
        ap_idle = 1'b1;
    end else begin
        ap_idle = 1'b0;
    end
end

always @ (*) begin
    if (((mmap_0_BVALID == 1'b1) & (1'b1 == ap_CS_fsm_state8))) begin
        ap_ready = 1'b1;
    end else begin
        ap_ready = 1'b0;
    end
end

always @ (*) begin
    if ((~((mmap_0_AWREADY == 1'b0) | (ap_start == 1'b0)) & (1'b1 == ap_CS_fsm_state1))) begin
        mmap_0_AWADDR = sext_ln32_fu_112_p1;
    end else if (((1'b1 == ap_CS_fsm_state3) | (1'b1 == ap_CS_fsm_state2))) begin
        mmap_0_AWADDR = grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_AWADDR;
    end else begin
        mmap_0_AWADDR = 'bx;
    end
end

always @ (*) begin
    if ((~((mmap_0_AWREADY == 1'b0) | (ap_start == 1'b0)) & (1'b1 == ap_CS_fsm_state1))) begin
        mmap_0_AWLEN = n;
    end else if (((1'b1 == ap_CS_fsm_state3) | (1'b1 == ap_CS_fsm_state2))) begin
        mmap_0_AWLEN = grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_AWLEN;
    end else begin
        mmap_0_AWLEN = 'bx;
    end
end

always @ (*) begin
    if ((~((mmap_0_AWREADY == 1'b0) | (ap_start == 1'b0)) & (1'b1 == ap_CS_fsm_state1))) begin
        mmap_0_AWVALID = 1'b1;
    end else if (((1'b1 == ap_CS_fsm_state3) | (1'b1 == ap_CS_fsm_state2))) begin
        mmap_0_AWVALID = grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_AWVALID;
    end else begin
        mmap_0_AWVALID = 1'b0;
    end
end

always @ (*) begin
    if (((mmap_0_BVALID == 1'b1) & (1'b1 == ap_CS_fsm_state8))) begin
        mmap_0_BREADY = 1'b1;
    end else if (((1'b1 == ap_CS_fsm_state3) | (1'b1 == ap_CS_fsm_state2))) begin
        mmap_0_BREADY = grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_BREADY;
    end else begin
        mmap_0_BREADY = 1'b0;
    end
end

always @ (*) begin
    if (((1'b1 == ap_CS_fsm_state3) | (1'b1 == ap_CS_fsm_state2))) begin
        mmap_0_WVALID = grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_m_axi_mmap_0_WVALID;
    end else begin
        mmap_0_WVALID = 1'b0;
    end
end

always @ (*) begin
    if (((1'b1 == ap_CS_fsm_state1) & (ap_start == 1'b1))) begin
        mmap_blk_n_AW = m_axi_mmap_AWREADY;
    end else begin
        mmap_blk_n_AW = 1'b1;
    end
end

always @ (*) begin
    if ((1'b1 == ap_CS_fsm_state8)) begin
        mmap_blk_n_B = m_axi_mmap_BVALID;
    end else begin
        mmap_blk_n_B = 1'b1;
    end
end

always @ (*) begin
    if ((1'b1 == ap_CS_fsm_state3)) begin
        stream_s_read = grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_stream_s_read;
    end else begin
        stream_s_read = 1'b0;
    end
end

always @ (*) begin
    case (ap_CS_fsm)
        ap_ST_fsm_state1 : begin
            if ((~((mmap_0_AWREADY == 1'b0) | (ap_start == 1'b0)) & (1'b1 == ap_CS_fsm_state1))) begin
                ap_NS_fsm = ap_ST_fsm_state2;
            end else begin
                ap_NS_fsm = ap_ST_fsm_state1;
            end
        end
        ap_ST_fsm_state2 : begin
            ap_NS_fsm = ap_ST_fsm_state3;
        end
        ap_ST_fsm_state3 : begin
            if (((1'b1 == ap_CS_fsm_state3) & (grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_ap_done == 1'b1))) begin
                ap_NS_fsm = ap_ST_fsm_state4;
            end else begin
                ap_NS_fsm = ap_ST_fsm_state3;
            end
        end
        ap_ST_fsm_state4 : begin
            ap_NS_fsm = ap_ST_fsm_state5;
        end
        ap_ST_fsm_state5 : begin
            ap_NS_fsm = ap_ST_fsm_state6;
        end
        ap_ST_fsm_state6 : begin
            ap_NS_fsm = ap_ST_fsm_state7;
        end
        ap_ST_fsm_state7 : begin
            ap_NS_fsm = ap_ST_fsm_state8;
        end
        ap_ST_fsm_state8 : begin
            if (((mmap_0_BVALID == 1'b1) & (1'b1 == ap_CS_fsm_state8))) begin
                ap_NS_fsm = ap_ST_fsm_state1;
            end else begin
                ap_NS_fsm = ap_ST_fsm_state8;
            end
        end
        default : begin
            ap_NS_fsm = 'bx;
        end
    endcase
end

assign ap_CS_fsm_state1 = ap_CS_fsm[32'd0];

assign ap_CS_fsm_state2 = ap_CS_fsm[32'd1];

assign ap_CS_fsm_state3 = ap_CS_fsm[32'd2];

assign ap_CS_fsm_state8 = ap_CS_fsm[32'd7];

assign ap_NS_fsm_state2 = ap_NS_fsm[32'd1];

always @ (*) begin
    ap_rst_n_inv = ~ap_rst_n;
end

assign grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_ap_start = grp_Stream2Mmap_Pipeline_VITIS_LOOP_32_1_fu_92_ap_start_reg;

assign sext_ln32_fu_112_p1 = trunc_ln_fu_102_p4;

assign stream_peek_read = 1'b0;

assign trunc_ln_fu_102_p4 = {{mmap_offset[63:2]}};

endmodule //Stream2Mmap
