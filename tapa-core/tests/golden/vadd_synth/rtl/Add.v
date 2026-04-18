module Add #(
  parameter ap_ST_fsm_state1 = 3'd1,
  parameter ap_ST_fsm_state2 = 3'd2,
  parameter ap_ST_fsm_state3 = 3'd4
) (
  input wire ap_clk,
  input wire ap_rst_n,
  input wire ap_start,
  output wire ap_done,
  output wire ap_idle,
  output wire ap_ready,
  input wire [32:0] a_s_dout,
  input wire a_s_empty_n,
  output wire a_s_read,
  input wire [32:0] a_peek_dout,
  input wire a_peek_empty_n,
  output wire a_peek_read,
  input wire [32:0] b_s_dout,
  input wire b_s_empty_n,
  output wire b_s_read,
  input wire [32:0] b_peek_dout,
  input wire b_peek_empty_n,
  output wire b_peek_read,
  output wire [32:0] c_s_din,
  input wire c_s_full_n,
  output wire c_s_write,
  input wire [32:0] c_peek,
  input wire [63:0] n
);

reg ap_done;
reg ap_idle;
reg ap_ready;
reg a_s_read;
reg b_s_read;
reg c_s_write;
reg ap_rst_n_inv;
reg [2:0] ap_CS_fsm;
wire ap_CS_fsm_state1;
reg [63:0] n_read_reg_100;
wire ap_CS_fsm_state2;
wire grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_ap_start;
wire grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_ap_done;
wire grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_ap_idle;
wire grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_ap_ready;
wire grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_a_s_read;
wire grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_b_s_read;
wire [32:0] grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_c_s_din;
wire grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_c_s_write;
reg grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_ap_start_reg;
wire ap_CS_fsm_state3;
reg [2:0] ap_NS_fsm;
reg ap_ST_fsm_state1_blk;
wire ap_ST_fsm_state2_blk;
reg ap_ST_fsm_state3_blk;
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
        grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_ap_start_reg <= 1'b0;
    end else begin
        if ((1'b1 == ap_CS_fsm_state2)) begin
            grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_ap_start_reg <= 1'b1;
        end else if ((grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_ap_ready == 1'b1)) begin
            grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_ap_start_reg <= 1'b0;
        end
    end
end

always @ (posedge ap_clk) begin
    if ((1'b1 == ap_CS_fsm_state2)) begin
        n_read_reg_100 <= n;
    end
end

always @ (*) begin
    if ((1'b1 == ap_CS_fsm_state3)) begin
        a_s_read = grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_a_s_read;
    end else begin
        a_s_read = 1'b0;
    end
end

always @ (*) begin
    if ((ap_start == 1'b0)) begin
        ap_ST_fsm_state1_blk = 1'b1;
    end else begin
        ap_ST_fsm_state1_blk = 1'b0;
    end
end

assign ap_ST_fsm_state2_blk = 1'b0;

always @ (*) begin
    if ((grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_ap_done == 1'b0)) begin
        ap_ST_fsm_state3_blk = 1'b1;
    end else begin
        ap_ST_fsm_state3_blk = 1'b0;
    end
end

always @ (*) begin
    if (((grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_ap_done == 1'b1) & (1'b1 == ap_CS_fsm_state3))) begin
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
    if (((grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_ap_done == 1'b1) & (1'b1 == ap_CS_fsm_state3))) begin
        ap_ready = 1'b1;
    end else begin
        ap_ready = 1'b0;
    end
end

always @ (*) begin
    if ((1'b1 == ap_CS_fsm_state3)) begin
        b_s_read = grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_b_s_read;
    end else begin
        b_s_read = 1'b0;
    end
end

always @ (*) begin
    if ((1'b1 == ap_CS_fsm_state3)) begin
        c_s_write = grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_c_s_write;
    end else begin
        c_s_write = 1'b0;
    end
end

always @ (*) begin
    case (ap_CS_fsm)
        ap_ST_fsm_state1 : begin
            if (((1'b1 == ap_CS_fsm_state1) & (ap_start == 1'b1))) begin
                ap_NS_fsm = ap_ST_fsm_state2;
            end else begin
                ap_NS_fsm = ap_ST_fsm_state1;
            end
        end
        ap_ST_fsm_state2 : begin
            ap_NS_fsm = ap_ST_fsm_state3;
        end
        ap_ST_fsm_state3 : begin
            if (((grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_ap_done == 1'b1) & (1'b1 == ap_CS_fsm_state3))) begin
                ap_NS_fsm = ap_ST_fsm_state1;
            end else begin
                ap_NS_fsm = ap_ST_fsm_state3;
            end
        end
        default : begin
            ap_NS_fsm = 'bx;
        end
    endcase
end

assign a_peek_read = 1'b0;

assign ap_CS_fsm_state1 = ap_CS_fsm[32'd0];

assign ap_CS_fsm_state2 = ap_CS_fsm[32'd1];

assign ap_CS_fsm_state3 = ap_CS_fsm[32'd2];

always @ (*) begin
    ap_rst_n_inv = ~ap_rst_n;
end

assign b_peek_read = 1'b0;

assign c_s_din = grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_c_s_din;

assign grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_ap_start = grp_Add_Pipeline_VITIS_LOOP_39_1_fu_88_ap_start_reg;

endmodule //Add
