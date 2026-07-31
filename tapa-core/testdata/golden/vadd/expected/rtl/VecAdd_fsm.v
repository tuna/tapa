module VecAdd_fsm (
  input wire ap_clk,
  input wire ap_rst_n,
  input wire ap_start,
  output wire ap_done,
  output wire ap_ready,
  output wire ap_idle,
  output wire Add_0__ap_start,
  input wire Add_0__ap_ready,
  input wire Add_0__ap_done,
  input wire Add_0__ap_idle,
  output wire Add_0__is_done,
  input wire [63:0] Add_0__n_in,
  output wire [63:0] Add_0__n,
  output wire Mmap2Stream_0__ap_start,
  input wire Mmap2Stream_0__ap_ready,
  input wire Mmap2Stream_0__ap_done,
  input wire Mmap2Stream_0__ap_idle,
  output wire Mmap2Stream_0__is_done,
  input wire [63:0] Mmap2Stream_0__mmap_port_offset_in,
  output wire [63:0] Mmap2Stream_0__mmap_port_offset,
  output wire Mmap2Stream_1__ap_start,
  input wire Mmap2Stream_1__ap_ready,
  input wire Mmap2Stream_1__ap_done,
  input wire Mmap2Stream_1__ap_idle,
  output wire Mmap2Stream_1__is_done,
  input wire [63:0] Mmap2Stream_1__mmap_port_offset_in,
  output wire [63:0] Mmap2Stream_1__mmap_port_offset,
  output wire Stream2Mmap_0__ap_start,
  input wire Stream2Mmap_0__ap_ready,
  input wire Stream2Mmap_0__ap_done,
  input wire Stream2Mmap_0__ap_idle,
  output wire Stream2Mmap_0__is_done,
  input wire [63:0] Stream2Mmap_0__mmap_port_offset_in,
  output wire [63:0] Stream2Mmap_0__mmap_port_offset
);

reg [1:0] Add_0__state;
reg [63:0] Add_0__n_reg;
reg [1:0] Mmap2Stream_0__state;
reg [63:0] Mmap2Stream_0__mmap_port_offset_reg;
reg [1:0] Mmap2Stream_1__state;
reg [63:0] Mmap2Stream_1__mmap_port_offset_reg;
reg [1:0] Stream2Mmap_0__state;
reg [63:0] Stream2Mmap_0__mmap_port_offset_reg;
reg [1:0] __tapa_state;
wire ap_rst;
wire __tapa_start_q;
wire __tapa_done_q;

assign Add_0__ap_start = (Add_0__state == 2'b01);
assign Add_0__is_done = (Add_0__state == 2'b10);
assign Add_0__n = Add_0__n_reg;
assign Mmap2Stream_0__ap_start = (Mmap2Stream_0__state == 2'b01);
assign Mmap2Stream_0__is_done = (Mmap2Stream_0__state == 2'b10);
assign Mmap2Stream_0__mmap_port_offset = Mmap2Stream_0__mmap_port_offset_reg;
assign Mmap2Stream_1__ap_start = (Mmap2Stream_1__state == 2'b01);
assign Mmap2Stream_1__is_done = (Mmap2Stream_1__state == 2'b10);
assign Mmap2Stream_1__mmap_port_offset = Mmap2Stream_1__mmap_port_offset_reg;
assign Stream2Mmap_0__ap_start = (Stream2Mmap_0__state == 2'b01);
assign Stream2Mmap_0__is_done = (Stream2Mmap_0__state == 2'b10);
assign Stream2Mmap_0__mmap_port_offset = Stream2Mmap_0__mmap_port_offset_reg;
assign ap_rst = !ap_rst_n;
assign __tapa_start_q = ap_start;
assign ap_idle = (__tapa_state == 2'b00);
assign __tapa_done_q = (__tapa_state == 2'b10);
assign ap_done = __tapa_done_q;
assign ap_ready = __tapa_done_q;
always @(posedge ap_clk) begin
  if (ap_rst) begin
    Add_0__state <= 2'b00;
  end else begin
    case (Add_0__state)
      2'b00: begin
        if (__tapa_start_q) begin
          Add_0__state <= 2'b01;
        end
      end
      2'b01: begin
        if ((Add_0__ap_ready && Add_0__ap_done)) begin
          Add_0__state <= 2'b10;
        end else begin
          if (Add_0__ap_ready) begin
            Add_0__state <= 2'b11;
          end
        end
      end
      2'b11: begin
        if (Add_0__ap_done) begin
          Add_0__state <= 2'b10;
        end
      end
      2'b10: begin
        if (__tapa_done_q) begin
          Add_0__state <= 2'b00;
        end
      end
      default: begin
        Add_0__state <= 2'b00;
      end
    endcase
  end
end

always @(posedge ap_clk) begin
  Add_0__n_reg <= Add_0__n_in;
end

always @(posedge ap_clk) begin
  if (ap_rst) begin
    Mmap2Stream_0__state <= 2'b00;
  end else begin
    case (Mmap2Stream_0__state)
      2'b00: begin
        if (__tapa_start_q) begin
          Mmap2Stream_0__state <= 2'b01;
        end
      end
      2'b01: begin
        if ((Mmap2Stream_0__ap_ready && Mmap2Stream_0__ap_done)) begin
          Mmap2Stream_0__state <= 2'b10;
        end else begin
          if (Mmap2Stream_0__ap_ready) begin
            Mmap2Stream_0__state <= 2'b11;
          end
        end
      end
      2'b11: begin
        if (Mmap2Stream_0__ap_done) begin
          Mmap2Stream_0__state <= 2'b10;
        end
      end
      2'b10: begin
        if (__tapa_done_q) begin
          Mmap2Stream_0__state <= 2'b00;
        end
      end
      default: begin
        Mmap2Stream_0__state <= 2'b00;
      end
    endcase
  end
end

always @(posedge ap_clk) begin
  Mmap2Stream_0__mmap_port_offset_reg <= Mmap2Stream_0__mmap_port_offset_in;
end

always @(posedge ap_clk) begin
  if (ap_rst) begin
    Mmap2Stream_1__state <= 2'b00;
  end else begin
    case (Mmap2Stream_1__state)
      2'b00: begin
        if (__tapa_start_q) begin
          Mmap2Stream_1__state <= 2'b01;
        end
      end
      2'b01: begin
        if ((Mmap2Stream_1__ap_ready && Mmap2Stream_1__ap_done)) begin
          Mmap2Stream_1__state <= 2'b10;
        end else begin
          if (Mmap2Stream_1__ap_ready) begin
            Mmap2Stream_1__state <= 2'b11;
          end
        end
      end
      2'b11: begin
        if (Mmap2Stream_1__ap_done) begin
          Mmap2Stream_1__state <= 2'b10;
        end
      end
      2'b10: begin
        if (__tapa_done_q) begin
          Mmap2Stream_1__state <= 2'b00;
        end
      end
      default: begin
        Mmap2Stream_1__state <= 2'b00;
      end
    endcase
  end
end

always @(posedge ap_clk) begin
  Mmap2Stream_1__mmap_port_offset_reg <= Mmap2Stream_1__mmap_port_offset_in;
end

always @(posedge ap_clk) begin
  if (ap_rst) begin
    Stream2Mmap_0__state <= 2'b00;
  end else begin
    case (Stream2Mmap_0__state)
      2'b00: begin
        if (__tapa_start_q) begin
          Stream2Mmap_0__state <= 2'b01;
        end
      end
      2'b01: begin
        if ((Stream2Mmap_0__ap_ready && Stream2Mmap_0__ap_done)) begin
          Stream2Mmap_0__state <= 2'b10;
        end else begin
          if (Stream2Mmap_0__ap_ready) begin
            Stream2Mmap_0__state <= 2'b11;
          end
        end
      end
      2'b11: begin
        if (Stream2Mmap_0__ap_done) begin
          Stream2Mmap_0__state <= 2'b10;
        end
      end
      2'b10: begin
        if (__tapa_done_q) begin
          Stream2Mmap_0__state <= 2'b00;
        end
      end
      default: begin
        Stream2Mmap_0__state <= 2'b00;
      end
    endcase
  end
end

always @(posedge ap_clk) begin
  Stream2Mmap_0__mmap_port_offset_reg <= Stream2Mmap_0__mmap_port_offset_in;
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
        if ((((Add_0__is_done && Mmap2Stream_0__is_done) && Mmap2Stream_1__is_done) && Stream2Mmap_0__is_done)) begin
          __tapa_state <= 2'b10;
        end
      end
      2'b10: begin
        __tapa_state <= 2'b00;
      end
    endcase
  end
end

endmodule //VecAdd_fsm
