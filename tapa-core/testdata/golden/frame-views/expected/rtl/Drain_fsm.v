module Drain_fsm (
  input wire ap_clk,
  input wire ap_rst_n,
  input wire ap_start,
  output wire ap_done,
  output wire ap_ready,
  output wire ap_idle,
  output wire Store_0__ap_start,
  input wire Store_0__ap_ready,
  input wire Store_0__ap_done,
  input wire Store_0__ap_idle,
  output wire Store_0__is_done,
  input wire [63:0] Store_0__dst_offset_in,
  output wire [63:0] Store_0__dst_offset,
  input wire [31:0] Store_0__n_in,
  output wire [31:0] Store_0__n,
  output wire TurnIn_0__ap_start,
  input wire TurnIn_0__ap_ready,
  input wire TurnIn_0__ap_done,
  input wire TurnIn_0__ap_idle,
  output wire TurnIn_0__is_done,
  input wire [31:0] TurnIn_0__n_in,
  output wire [31:0] TurnIn_0__n
);

reg [1:0] Store_0__state;
reg [63:0] Store_0__dst_offset_reg;
reg [31:0] Store_0__n_reg;
reg [1:0] TurnIn_0__state;
reg [31:0] TurnIn_0__n_reg;
reg [1:0] __tapa_state;
wire ap_rst;
wire __tapa_start_q;
wire __tapa_done_q;

assign Store_0__ap_start = (Store_0__state == 2'b01);
assign Store_0__is_done = (Store_0__state == 2'b10);
assign Store_0__dst_offset = Store_0__dst_offset_reg;
assign Store_0__n = Store_0__n_reg;
assign TurnIn_0__ap_start = (TurnIn_0__state == 2'b01);
assign TurnIn_0__is_done = (TurnIn_0__state == 2'b10);
assign TurnIn_0__n = TurnIn_0__n_reg;
assign ap_rst = !ap_rst_n;
assign __tapa_start_q = ap_start;
assign ap_idle = (__tapa_state == 2'b00);
assign __tapa_done_q = (__tapa_state == 2'b10);
assign ap_done = __tapa_done_q;
assign ap_ready = __tapa_done_q;
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
    TurnIn_0__state <= 2'b00;
  end else begin
    case (TurnIn_0__state)
      2'b00: begin
        if (__tapa_start_q) begin
          TurnIn_0__state <= 2'b01;
        end
      end
      2'b01: begin
        if ((TurnIn_0__ap_ready && TurnIn_0__ap_done)) begin
          TurnIn_0__state <= 2'b10;
        end else begin
          if (TurnIn_0__ap_ready) begin
            TurnIn_0__state <= 2'b11;
          end
        end
      end
      2'b11: begin
        if (TurnIn_0__ap_done) begin
          TurnIn_0__state <= 2'b10;
        end
      end
      2'b10: begin
        if (__tapa_done_q) begin
          TurnIn_0__state <= 2'b00;
        end
      end
      default: begin
        TurnIn_0__state <= 2'b00;
      end
    endcase
  end
end

always @(posedge ap_clk) begin
  TurnIn_0__n_reg <= TurnIn_0__n_in;
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
        if ((Store_0__is_done && TurnIn_0__is_done)) begin
          __tapa_state <= 2'b10;
        end
      end
      2'b10: begin
        __tapa_state <= 2'b00;
      end
    endcase
  end
end

endmodule //Drain_fsm
