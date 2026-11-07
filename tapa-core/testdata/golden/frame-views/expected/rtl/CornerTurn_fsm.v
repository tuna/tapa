module CornerTurn_fsm (
  input wire ap_clk,
  input wire ap_rst_n,
  input wire ap_start,
  output wire ap_done,
  output wire ap_ready,
  output wire ap_idle,
  output wire Load_0__ap_start,
  input wire Load_0__ap_ready,
  input wire Load_0__ap_done,
  input wire Load_0__ap_idle,
  output wire Load_0__is_done,
  input wire [31:0] Load_0__n_in,
  output wire [31:0] Load_0__n,
  input wire [63:0] Load_0__src_offset_in,
  output wire [63:0] Load_0__src_offset,
  output wire Store_0__ap_start,
  input wire Store_0__ap_ready,
  input wire Store_0__ap_done,
  input wire Store_0__ap_idle,
  output wire Store_0__is_done,
  input wire [63:0] Store_0__dst_offset_in,
  output wire [63:0] Store_0__dst_offset,
  input wire [31:0] Store_0__n_in,
  output wire [31:0] Store_0__n,
  output wire Turn_0__ap_start,
  input wire Turn_0__ap_ready,
  input wire Turn_0__ap_done,
  input wire Turn_0__ap_idle,
  output wire Turn_0__is_done,
  input wire [31:0] Turn_0__n_in,
  output wire [31:0] Turn_0__n,
  output wire Turn_1__ap_start,
  input wire Turn_1__ap_ready,
  input wire Turn_1__ap_done,
  input wire Turn_1__ap_idle,
  output wire Turn_1__is_done,
  input wire [31:0] Turn_1__n_in,
  output wire [31:0] Turn_1__n,
  output wire Turn_2__ap_start,
  input wire Turn_2__ap_ready,
  input wire Turn_2__ap_done,
  input wire Turn_2__ap_idle,
  output wire Turn_2__is_done,
  input wire [31:0] Turn_2__n_in,
  output wire [31:0] Turn_2__n
);

reg [1:0] Load_0__state;
reg [31:0] Load_0__n_reg;
reg [63:0] Load_0__src_offset_reg;
reg [1:0] Store_0__state;
reg [63:0] Store_0__dst_offset_reg;
reg [31:0] Store_0__n_reg;
reg [1:0] Turn_0__state;
reg [31:0] Turn_0__n_reg;
reg [1:0] Turn_1__state;
reg [31:0] Turn_1__n_reg;
reg [1:0] Turn_2__state;
reg [31:0] Turn_2__n_reg;
reg [1:0] __tapa_state;
wire ap_rst;
wire __tapa_start_q;
wire __tapa_done_q;

assign Load_0__ap_start = (Load_0__state == 2'b01);
assign Load_0__is_done = (Load_0__state == 2'b10);
assign Load_0__n = Load_0__n_reg;
assign Load_0__src_offset = Load_0__src_offset_reg;
assign Store_0__ap_start = (Store_0__state == 2'b01);
assign Store_0__is_done = (Store_0__state == 2'b10);
assign Store_0__dst_offset = Store_0__dst_offset_reg;
assign Store_0__n = Store_0__n_reg;
assign Turn_0__ap_start = (Turn_0__state == 2'b01);
assign Turn_0__is_done = (Turn_0__state == 2'b10);
assign Turn_0__n = Turn_0__n_reg;
assign Turn_1__ap_start = (Turn_1__state == 2'b01);
assign Turn_1__is_done = (Turn_1__state == 2'b10);
assign Turn_1__n = Turn_1__n_reg;
assign Turn_2__ap_start = (Turn_2__state == 2'b01);
assign Turn_2__is_done = (Turn_2__state == 2'b10);
assign Turn_2__n = Turn_2__n_reg;
assign ap_rst = !ap_rst_n;
assign __tapa_start_q = ap_start;
assign ap_idle = (__tapa_state == 2'b00);
assign __tapa_done_q = (__tapa_state == 2'b10);
assign ap_done = __tapa_done_q;
assign ap_ready = __tapa_done_q;
always @(posedge ap_clk) begin
  if (ap_rst) begin
    Load_0__state <= 2'b00;
  end else begin
    case (Load_0__state)
      2'b00: begin
        if (__tapa_start_q) begin
          Load_0__state <= 2'b01;
        end
      end
      2'b01: begin
        if ((Load_0__ap_ready && Load_0__ap_done)) begin
          Load_0__state <= 2'b10;
        end else begin
          if (Load_0__ap_ready) begin
            Load_0__state <= 2'b11;
          end
        end
      end
      2'b11: begin
        if (Load_0__ap_done) begin
          Load_0__state <= 2'b10;
        end
      end
      2'b10: begin
        if (__tapa_done_q) begin
          Load_0__state <= 2'b00;
        end
      end
      default: begin
        Load_0__state <= 2'b00;
      end
    endcase
  end
end

always @(posedge ap_clk) begin
  Load_0__n_reg <= Load_0__n_in;
end

always @(posedge ap_clk) begin
  Load_0__src_offset_reg <= Load_0__src_offset_in;
end

always @(posedge ap_clk) begin
  if (ap_rst) begin
    Store_0__state <= 2'b00;
  end else begin
    case (Store_0__state)
      2'b00: begin
        if (__tapa_start_q) begin
          Store_0__state <= 2'b01;
        end
      end
      2'b01: begin
        if ((Store_0__ap_ready && Store_0__ap_done)) begin
          Store_0__state <= 2'b10;
        end else begin
          if (Store_0__ap_ready) begin
            Store_0__state <= 2'b11;
          end
        end
      end
      2'b11: begin
        if (Store_0__ap_done) begin
          Store_0__state <= 2'b10;
        end
      end
      2'b10: begin
        if (__tapa_done_q) begin
          Store_0__state <= 2'b00;
        end
      end
      default: begin
        Store_0__state <= 2'b00;
      end
    endcase
  end
end

always @(posedge ap_clk) begin
  Store_0__dst_offset_reg <= Store_0__dst_offset_in;
end

always @(posedge ap_clk) begin
  Store_0__n_reg <= Store_0__n_in;
end

always @(posedge ap_clk) begin
  if (ap_rst) begin
    Turn_0__state <= 2'b00;
  end else begin
    case (Turn_0__state)
      2'b00: begin
        if (__tapa_start_q) begin
          Turn_0__state <= 2'b01;
        end
      end
      2'b01: begin
        if ((Turn_0__ap_ready && Turn_0__ap_done)) begin
          Turn_0__state <= 2'b10;
        end else begin
          if (Turn_0__ap_ready) begin
            Turn_0__state <= 2'b11;
          end
        end
      end
      2'b11: begin
        if (Turn_0__ap_done) begin
          Turn_0__state <= 2'b10;
        end
      end
      2'b10: begin
        if (__tapa_done_q) begin
          Turn_0__state <= 2'b00;
        end
      end
      default: begin
        Turn_0__state <= 2'b00;
      end
    endcase
  end
end

always @(posedge ap_clk) begin
  Turn_0__n_reg <= Turn_0__n_in;
end

always @(posedge ap_clk) begin
  if (ap_rst) begin
    Turn_1__state <= 2'b00;
  end else begin
    case (Turn_1__state)
      2'b00: begin
        if (__tapa_start_q) begin
          Turn_1__state <= 2'b01;
        end
      end
      2'b01: begin
        if ((Turn_1__ap_ready && Turn_1__ap_done)) begin
          Turn_1__state <= 2'b10;
        end else begin
          if (Turn_1__ap_ready) begin
            Turn_1__state <= 2'b11;
          end
        end
      end
      2'b11: begin
        if (Turn_1__ap_done) begin
          Turn_1__state <= 2'b10;
        end
      end
      2'b10: begin
        if (__tapa_done_q) begin
          Turn_1__state <= 2'b00;
        end
      end
      default: begin
        Turn_1__state <= 2'b00;
      end
    endcase
  end
end

always @(posedge ap_clk) begin
  Turn_1__n_reg <= Turn_1__n_in;
end

always @(posedge ap_clk) begin
  if (ap_rst) begin
    Turn_2__state <= 2'b00;
  end else begin
    case (Turn_2__state)
      2'b00: begin
        if (__tapa_start_q) begin
          Turn_2__state <= 2'b01;
        end
      end
      2'b01: begin
        if ((Turn_2__ap_ready && Turn_2__ap_done)) begin
          Turn_2__state <= 2'b10;
        end else begin
          if (Turn_2__ap_ready) begin
            Turn_2__state <= 2'b11;
          end
        end
      end
      2'b11: begin
        if (Turn_2__ap_done) begin
          Turn_2__state <= 2'b10;
        end
      end
      2'b10: begin
        if (__tapa_done_q) begin
          Turn_2__state <= 2'b00;
        end
      end
      default: begin
        Turn_2__state <= 2'b00;
      end
    endcase
  end
end

always @(posedge ap_clk) begin
  Turn_2__n_reg <= Turn_2__n_in;
end

always @(posedge ap_clk) begin
  if (ap_rst) begin
    __tapa_state <= 2'b00;
  end else begin
    case (__tapa_state)
      2'b00: begin
        if (ap_start) begin
          __tapa_state <= 2'b01;
        end
      end
      2'b01: begin
        if (((((Load_0__is_done && Store_0__is_done) && Turn_0__is_done) && Turn_1__is_done) && Turn_2__is_done)) begin
          __tapa_state <= 2'b10;
        end
      end
      2'b10: begin
        __tapa_state <= 2'b00;
      end
    endcase
  end
end

endmodule //CornerTurn_fsm
