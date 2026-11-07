module CornerTurn_fsm (
  input wire ap_clk,
  input wire ap_rst_n,
  input wire ap_start,
  output wire ap_done,
  output wire ap_ready,
  output wire ap_idle,
  output wire Drain_0__ap_start,
  input wire Drain_0__ap_ready,
  input wire Drain_0__ap_done,
  input wire Drain_0__ap_idle,
  output wire Drain_0__is_done,
  input wire [63:0] Drain_0__dst_offset_in,
  output wire [63:0] Drain_0__dst_offset,
  input wire [31:0] Drain_0__n_in,
  output wire [31:0] Drain_0__n,
  output wire Emit_0__ap_start,
  input wire Emit_0__ap_ready,
  input wire Emit_0__ap_done,
  input wire Emit_0__ap_idle,
  output wire Emit_0__is_done,
  input wire [31:0] Emit_0__n_in,
  output wire [31:0] Emit_0__n,
  output wire Load_0__ap_start,
  input wire Load_0__ap_ready,
  input wire Load_0__ap_done,
  input wire Load_0__ap_idle,
  output wire Load_0__is_done,
  input wire [31:0] Load_0__n_in,
  output wire [31:0] Load_0__n,
  input wire [63:0] Load_0__src_offset_in,
  output wire [63:0] Load_0__src_offset,
  output wire Turn_0__ap_start,
  input wire Turn_0__ap_ready,
  input wire Turn_0__ap_done,
  input wire Turn_0__ap_idle,
  output wire Turn_0__is_done,
  input wire [31:0] Turn_0__n_in,
  output wire [31:0] Turn_0__n
);

reg [1:0] Drain_0__state;
reg [63:0] Drain_0__dst_offset_reg;
reg [31:0] Drain_0__n_reg;
reg [1:0] Emit_0__state;
reg [31:0] Emit_0__n_reg;
reg [1:0] Load_0__state;
reg [31:0] Load_0__n_reg;
reg [63:0] Load_0__src_offset_reg;
reg [1:0] Turn_0__state;
reg [31:0] Turn_0__n_reg;
reg [1:0] __tapa_state;
wire ap_rst;
wire __tapa_start_q;
wire __tapa_done_q;

assign Drain_0__ap_start = (Drain_0__state == 2'b01);
assign Drain_0__is_done = (Drain_0__state == 2'b10);
assign Drain_0__dst_offset = Drain_0__dst_offset_reg;
assign Drain_0__n = Drain_0__n_reg;
assign Emit_0__ap_start = (Emit_0__state == 2'b01);
assign Emit_0__is_done = (Emit_0__state == 2'b10);
assign Emit_0__n = Emit_0__n_reg;
assign Load_0__ap_start = (Load_0__state == 2'b01);
assign Load_0__is_done = (Load_0__state == 2'b10);
assign Load_0__n = Load_0__n_reg;
assign Load_0__src_offset = Load_0__src_offset_reg;
assign Turn_0__ap_start = (Turn_0__state == 2'b01);
assign Turn_0__is_done = (Turn_0__state == 2'b10);
assign Turn_0__n = Turn_0__n_reg;
assign ap_rst = !ap_rst_n;
assign __tapa_start_q = ap_start;
assign ap_idle = (__tapa_state == 2'b00);
assign __tapa_done_q = (__tapa_state == 2'b10);
assign ap_done = __tapa_done_q;
assign ap_ready = __tapa_done_q;
always @(posedge ap_clk) begin
  if (ap_rst) begin
    Drain_0__state <= 2'b00;
  end else begin
    case (Drain_0__state)
      2'b00: begin
        if (__tapa_start_q) begin
          Drain_0__state <= 2'b01;
        end
      end
      2'b01: begin
        if ((Drain_0__ap_ready && Drain_0__ap_done)) begin
          Drain_0__state <= 2'b10;
        end else begin
          if (Drain_0__ap_ready) begin
            Drain_0__state <= 2'b11;
          end
        end
      end
      2'b11: begin
        if (Drain_0__ap_done) begin
          Drain_0__state <= 2'b10;
        end
      end
      2'b10: begin
        if (__tapa_done_q) begin
          Drain_0__state <= 2'b00;
        end
      end
      default: begin
        Drain_0__state <= 2'b00;
      end
    endcase
  end
end

always @(posedge ap_clk) begin
  Drain_0__dst_offset_reg <= Drain_0__dst_offset_in;
end

always @(posedge ap_clk) begin
  Drain_0__n_reg <= Drain_0__n_in;
end

always @(posedge ap_clk) begin
  if (ap_rst) begin
    Emit_0__state <= 2'b00;
  end else begin
    case (Emit_0__state)
      2'b00: begin
        if (__tapa_start_q) begin
          Emit_0__state <= 2'b01;
        end
      end
      2'b01: begin
        if ((Emit_0__ap_ready && Emit_0__ap_done)) begin
          Emit_0__state <= 2'b10;
        end else begin
          if (Emit_0__ap_ready) begin
            Emit_0__state <= 2'b11;
          end
        end
      end
      2'b11: begin
        if (Emit_0__ap_done) begin
          Emit_0__state <= 2'b10;
        end
      end
      2'b10: begin
        if (__tapa_done_q) begin
          Emit_0__state <= 2'b00;
        end
      end
      default: begin
        Emit_0__state <= 2'b00;
      end
    endcase
  end
end

always @(posedge ap_clk) begin
  Emit_0__n_reg <= Emit_0__n_in;
end

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
    __tapa_state <= 2'b00;
  end else begin
    case (__tapa_state)
      2'b00: begin
        if (ap_start) begin
          __tapa_state <= 2'b01;
        end
      end
      2'b01: begin
        if ((((Drain_0__is_done && Emit_0__is_done) && Load_0__is_done) && Turn_0__is_done)) begin
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
