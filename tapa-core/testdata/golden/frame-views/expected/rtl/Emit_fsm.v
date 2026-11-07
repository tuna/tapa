module Emit_fsm (
  input wire ap_clk,
  input wire ap_rst_n,
  input wire ap_start,
  output wire ap_done,
  output wire ap_ready,
  output wire ap_idle,
  output wire TurnOut_0__ap_start,
  input wire TurnOut_0__ap_ready,
  input wire TurnOut_0__ap_done,
  input wire TurnOut_0__ap_idle,
  output wire TurnOut_0__is_done,
  input wire [31:0] TurnOut_0__n_in,
  output wire [31:0] TurnOut_0__n
);

reg [1:0] TurnOut_0__state;
reg [31:0] TurnOut_0__n_reg;
reg [1:0] __tapa_state;
wire ap_rst;
wire __tapa_start_q;
wire __tapa_done_q;

assign TurnOut_0__ap_start = (TurnOut_0__state == 2'b01);
assign TurnOut_0__is_done = (TurnOut_0__state == 2'b10);
assign TurnOut_0__n = TurnOut_0__n_reg;
assign ap_rst = !ap_rst_n;
assign __tapa_start_q = ap_start;
assign ap_idle = (__tapa_state == 2'b00);
assign __tapa_done_q = (__tapa_state == 2'b10);
assign ap_done = __tapa_done_q;
assign ap_ready = __tapa_done_q;
always @(posedge ap_clk) begin
  if (ap_rst) begin
    TurnOut_0__state <= 2'b00;
  end else begin
    case (TurnOut_0__state)
      2'b00: begin
        if (__tapa_start_q) begin
          TurnOut_0__state <= 2'b01;
        end
      end
      2'b01: begin
        if ((TurnOut_0__ap_ready && TurnOut_0__ap_done)) begin
          TurnOut_0__state <= 2'b10;
        end else begin
          if (TurnOut_0__ap_ready) begin
            TurnOut_0__state <= 2'b11;
          end
        end
      end
      2'b11: begin
        if (TurnOut_0__ap_done) begin
          TurnOut_0__state <= 2'b10;
        end
      end
      2'b10: begin
        if (__tapa_done_q) begin
          TurnOut_0__state <= 2'b00;
        end
      end
      default: begin
        TurnOut_0__state <= 2'b00;
      end
    endcase
  end
end

always @(posedge ap_clk) begin
  TurnOut_0__n_reg <= TurnOut_0__n_in;
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
        if (TurnOut_0__is_done) begin
          __tapa_state <= 2'b10;
        end
      end
      2'b10: begin
        __tapa_state <= 2'b00;
      end
    endcase
  end
end

endmodule //Emit_fsm
