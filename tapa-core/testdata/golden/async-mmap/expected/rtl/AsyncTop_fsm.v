module AsyncTop_fsm (
  input wire ap_clk,
  input wire ap_rst_n,
  input wire ap_start,
  output wire ap_done,
  output wire ap_ready,
  output wire ap_idle,
  output wire AsyncReader_0__ap_start,
  input wire AsyncReader_0__ap_ready,
  input wire AsyncReader_0__ap_done,
  input wire AsyncReader_0__ap_idle,
  output wire AsyncReader_0__is_done,
  input wire [63:0] AsyncReader_0__mem_offset_in,
  output wire [63:0] AsyncReader_0__mem_offset,
  input wire [63:0] AsyncReader_0__n_in,
  output wire [63:0] AsyncReader_0__n
);

reg [1:0] AsyncReader_0__state;
reg [63:0] AsyncReader_0__mem_offset_reg;
reg [63:0] AsyncReader_0__n_reg;
reg [1:0] __tapa_state;
wire ap_rst;
wire __tapa_start_q;
wire __tapa_done_q;

assign AsyncReader_0__ap_start = (AsyncReader_0__state == 2'b01);
assign AsyncReader_0__is_done = (AsyncReader_0__state == 2'b10);
assign AsyncReader_0__mem_offset = AsyncReader_0__mem_offset_reg;
assign AsyncReader_0__n = AsyncReader_0__n_reg;
assign ap_rst = !ap_rst_n;
assign __tapa_start_q = ap_start;
assign ap_idle = (__tapa_state == 2'b00);
assign __tapa_done_q = (__tapa_state == 2'b10);
assign ap_done = __tapa_done_q;
assign ap_ready = __tapa_done_q;
always @(posedge ap_clk) begin
  if (ap_rst) begin
    AsyncReader_0__state <= 2'b00;
  end else begin
    case (AsyncReader_0__state)
      2'b00: begin
        if (__tapa_start_q) begin
          AsyncReader_0__state <= 2'b01;
        end
      end
      2'b01: begin
        if ((AsyncReader_0__ap_ready && AsyncReader_0__ap_done)) begin
          AsyncReader_0__state <= 2'b10;
        end else begin
          if (AsyncReader_0__ap_ready) begin
            AsyncReader_0__state <= 2'b11;
          end
        end
      end
      2'b11: begin
        if (AsyncReader_0__ap_done) begin
          AsyncReader_0__state <= 2'b10;
        end
      end
      2'b10: begin
        if (__tapa_done_q) begin
          AsyncReader_0__state <= 2'b00;
        end
      end
      default: begin
        AsyncReader_0__state <= 2'b00;
      end
    endcase
  end
end

always @(posedge ap_clk) begin
  AsyncReader_0__mem_offset_reg <= AsyncReader_0__mem_offset_in;
end

always @(posedge ap_clk) begin
  AsyncReader_0__n_reg <= AsyncReader_0__n_in;
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
        if (AsyncReader_0__is_done) begin
          __tapa_state <= 2'b10;
        end
      end
      2'b10: begin
        __tapa_state <= 2'b00;
      end
    endcase
  end
end

endmodule //AsyncTop_fsm
