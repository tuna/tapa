module ShellTop_fsm (
  input wire ap_clk,
  input wire ap_rst_n,
  input wire ap_start,
  output wire ap_done,
  output wire ap_ready,
  output wire ap_idle,
  output wire CustomShell_0__ap_start,
  input wire CustomShell_0__ap_ready,
  input wire CustomShell_0__ap_done,
  input wire CustomShell_0__ap_idle,
  output wire CustomShell_0__is_done,
  input wire [63:0] CustomShell_0__n_in,
  output wire [63:0] CustomShell_0__n
);

reg [1:0] CustomShell_0__state;
reg [63:0] CustomShell_0__n_reg;
reg [1:0] __tapa_state;
wire ap_rst;
wire __tapa_start_q;
wire __tapa_done_q;

assign CustomShell_0__ap_start = (CustomShell_0__state == 2'b01);
assign CustomShell_0__is_done = (CustomShell_0__state == 2'b10);
assign CustomShell_0__n = CustomShell_0__n_reg;
assign ap_rst = !ap_rst_n;
assign __tapa_start_q = ap_start;
assign ap_idle = (__tapa_state == 2'b00);
assign __tapa_done_q = (__tapa_state == 2'b10);
assign ap_done = __tapa_done_q;
assign ap_ready = __tapa_done_q;
always @(posedge ap_clk) begin
  if (ap_rst) begin
    CustomShell_0__state <= 2'b00;
  end else begin
    case (CustomShell_0__state)
      2'b00: begin
        if (__tapa_start_q) begin
          CustomShell_0__state <= 2'b01;
        end
      end
      2'b01: begin
        if ((CustomShell_0__ap_ready && CustomShell_0__ap_done)) begin
          CustomShell_0__state <= 2'b10;
        end else begin
          if (CustomShell_0__ap_ready) begin
            CustomShell_0__state <= 2'b11;
          end
        end
      end
      2'b11: begin
        if (CustomShell_0__ap_done) begin
          CustomShell_0__state <= 2'b10;
        end
      end
      2'b10: begin
        if (__tapa_done_q) begin
          CustomShell_0__state <= 2'b00;
        end
      end
      default: begin
        CustomShell_0__state <= 2'b00;
      end
    endcase
  end
end

always @(posedge ap_clk) begin
  CustomShell_0__n_reg <= CustomShell_0__n_in;
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
        if (CustomShell_0__is_done) begin
          __tapa_state <= 2'b10;
        end
      end
      2'b10: begin
        __tapa_state <= 2'b00;
      end
    endcase
  end
end

endmodule //ShellTop_fsm
