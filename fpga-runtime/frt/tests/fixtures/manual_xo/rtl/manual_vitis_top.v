module manual_vitis_top (
    input wire ap_clk,
    input wire ap_rst_n,

    input wire s_axi_control_AWVALID,
    output wire s_axi_control_AWREADY,
    input wire [7:0] s_axi_control_AWADDR,
    input wire s_axi_control_WVALID,
    output wire s_axi_control_WREADY,
    input wire [31:0] s_axi_control_WDATA,
    input wire [3:0] s_axi_control_WSTRB,
    input wire s_axi_control_ARVALID,
    output wire s_axi_control_ARREADY,
    input wire [7:0] s_axi_control_ARADDR,
    output reg s_axi_control_RVALID,
    input wire s_axi_control_RREADY,
    output reg [31:0] s_axi_control_RDATA,
    output reg [1:0] s_axi_control_RRESP,
    output reg s_axi_control_BVALID,
    input wire s_axi_control_BREADY,
    output reg [1:0] s_axi_control_BRESP,
    output reg interrupt,

    output reg [63:0] m_axi_a_ARADDR,
    output reg [1:0] m_axi_a_ARBURST,
    output reg [3:0] m_axi_a_ARCACHE,
    output reg [0:0] m_axi_a_ARID,
    output reg [7:0] m_axi_a_ARLEN,
    output reg [1:0] m_axi_a_ARLOCK,
    output reg [2:0] m_axi_a_ARPROT,
    output reg [3:0] m_axi_a_ARQOS,
    input wire m_axi_a_ARREADY,
    output reg [2:0] m_axi_a_ARSIZE,
    output reg m_axi_a_ARVALID,

    output reg [63:0] m_axi_a_AWADDR,
    output reg [1:0] m_axi_a_AWBURST,
    output reg [3:0] m_axi_a_AWCACHE,
    output reg [0:0] m_axi_a_AWID,
    output reg [7:0] m_axi_a_AWLEN,
    output reg [1:0] m_axi_a_AWLOCK,
    output reg [2:0] m_axi_a_AWPROT,
    output reg [3:0] m_axi_a_AWQOS,
    input wire m_axi_a_AWREADY,
    output reg [2:0] m_axi_a_AWSIZE,
    output reg m_axi_a_AWVALID,

    input wire [0:0] m_axi_a_BID,
    output reg m_axi_a_BREADY,
    input wire [1:0] m_axi_a_BRESP,
    input wire m_axi_a_BVALID,

    input wire [31:0] m_axi_a_RDATA,
    input wire [0:0] m_axi_a_RID,
    input wire m_axi_a_RLAST,
    output reg m_axi_a_RREADY,
    input wire [1:0] m_axi_a_RRESP,
    input wire m_axi_a_RVALID,

    output reg [31:0] m_axi_a_WDATA,
    output reg m_axi_a_WLAST,
    input wire m_axi_a_WREADY,
    output reg [3:0] m_axi_a_WSTRB,
    output reg m_axi_a_WVALID,

    input wire [31:0] s_TDATA,
    input wire s_TVALID,
    output reg s_TREADY,
    input wire s_TLAST
);

  assign s_axi_control_AWREADY = 1'b1;
  assign s_axi_control_WREADY = 1'b1;
  assign s_axi_control_ARREADY = 1'b1;

  localparam ST_IDLE = 3'd0;
  localparam ST_AR = 3'd1;
  localparam ST_R = 3'd2;
  localparam ST_STREAM = 3'd3;
  localparam ST_AW_W = 3'd4;
  localparam ST_B = 3'd5;
  localparam ST_DONE = 3'd6;

  reg [2:0] state;
  reg [63:0] a_base;
  reg [31:0] scalar_n;
  reg start_cmd;
  reg [31:0] read_word;
  reg [31:0] stream_word;
  reg aw_sent;
  reg w_sent;

  always @(posedge ap_clk) begin
    if (!ap_rst_n) begin
      s_axi_control_RVALID <= 1'b0;
      s_axi_control_RDATA <= 32'd0;
      s_axi_control_RRESP <= 2'b00;
      s_axi_control_BVALID <= 1'b0;
      s_axi_control_BRESP <= 2'b00;
      interrupt <= 1'b0;
      start_cmd <= 1'b0;
      a_base <= 64'd0;
      scalar_n <= 32'd0;

      state <= ST_IDLE;
      read_word <= 32'd0;
      stream_word <= 32'd0;
      aw_sent <= 1'b0;
      w_sent <= 1'b0;
      s_TREADY <= 1'b0;

      m_axi_a_ARADDR <= 64'd0;
      m_axi_a_ARBURST <= 2'b01;
      m_axi_a_ARCACHE <= 4'b0000;
      m_axi_a_ARID <= 1'b0;
      m_axi_a_ARLEN <= 8'd0;
      m_axi_a_ARLOCK <= 2'b00;
      m_axi_a_ARPROT <= 3'b000;
      m_axi_a_ARQOS <= 4'b0000;
      m_axi_a_ARSIZE <= 3'd2;
      m_axi_a_ARVALID <= 1'b0;

      m_axi_a_AWADDR <= 64'd0;
      m_axi_a_AWBURST <= 2'b01;
      m_axi_a_AWCACHE <= 4'b0000;
      m_axi_a_AWID <= 1'b0;
      m_axi_a_AWLEN <= 8'd0;
      m_axi_a_AWLOCK <= 2'b00;
      m_axi_a_AWPROT <= 3'b000;
      m_axi_a_AWQOS <= 4'b0000;
      m_axi_a_AWSIZE <= 3'd2;
      m_axi_a_AWVALID <= 1'b0;

      m_axi_a_BREADY <= 1'b0;
      m_axi_a_RREADY <= 1'b0;
      m_axi_a_WDATA <= 32'd0;
      m_axi_a_WLAST <= 1'b1;
      m_axi_a_WSTRB <= 4'hF;
      m_axi_a_WVALID <= 1'b0;
    end else begin
      if (s_axi_control_BVALID && s_axi_control_BREADY) begin
        s_axi_control_BVALID <= 1'b0;
      end
      if (s_axi_control_AWVALID && s_axi_control_WVALID && !s_axi_control_BVALID) begin
        case (s_axi_control_AWADDR)
          8'h00: begin
            if (s_axi_control_WDATA[0]) begin
              start_cmd <= 1'b1;
              interrupt <= 1'b0;
            end
          end
          8'h10: a_base[31:0] <= s_axi_control_WDATA;
          8'h14: a_base[63:32] <= s_axi_control_WDATA;
          8'h1c: scalar_n <= s_axi_control_WDATA;
          default: begin
          end
        endcase
        s_axi_control_BVALID <= 1'b1;
        s_axi_control_BRESP <= 2'b00;
      end

      if (s_axi_control_ARVALID) begin
        s_axi_control_RVALID <= 1'b1;
        s_axi_control_RDATA <= 32'd0;
        s_axi_control_RRESP <= 2'b00;
      end
      if (s_axi_control_RVALID && s_axi_control_RREADY) begin
        s_axi_control_RVALID <= 1'b0;
      end

      case (state)
        ST_IDLE: begin
          m_axi_a_ARVALID <= 1'b0;
          m_axi_a_AWVALID <= 1'b0;
          m_axi_a_WVALID <= 1'b0;
          m_axi_a_BREADY <= 1'b0;
          m_axi_a_RREADY <= 1'b0;
          s_TREADY <= 1'b0;
          aw_sent <= 1'b0;
          w_sent <= 1'b0;
          if (start_cmd) begin
            start_cmd <= 1'b0;
            state <= ST_AR;
          end
        end
        ST_AR: begin
          m_axi_a_ARADDR <= a_base;
          m_axi_a_ARVALID <= 1'b1;
          if (m_axi_a_ARVALID && m_axi_a_ARREADY) begin
            m_axi_a_ARVALID <= 1'b0;
            m_axi_a_RREADY <= 1'b1;
            state <= ST_R;
          end
        end
        ST_R: begin
          if (m_axi_a_RVALID && m_axi_a_RREADY) begin
            read_word <= m_axi_a_RDATA;
            m_axi_a_RREADY <= 1'b0;
            s_TREADY <= 1'b1;
            state <= ST_STREAM;
          end
        end
        ST_STREAM: begin
          if (s_TVALID && s_TREADY) begin
            stream_word <= s_TDATA;
            s_TREADY <= 1'b0;
            state <= ST_AW_W;
            aw_sent <= 1'b0;
            w_sent <= 1'b0;
          end
        end
        ST_AW_W: begin
          m_axi_a_AWADDR <= a_base;
          m_axi_a_AWVALID <= !aw_sent;
          if (m_axi_a_AWVALID && m_axi_a_AWREADY) begin
            aw_sent <= 1'b1;
            m_axi_a_AWVALID <= 1'b0;
          end

          m_axi_a_WDATA <= read_word + stream_word + scalar_n;
          m_axi_a_WVALID <= !w_sent;
          if (m_axi_a_WVALID && m_axi_a_WREADY) begin
            w_sent <= 1'b1;
            m_axi_a_WVALID <= 1'b0;
          end

          if (aw_sent && w_sent) begin
            m_axi_a_BREADY <= 1'b1;
            state <= ST_B;
          end
        end
        ST_B: begin
          if (m_axi_a_BVALID && m_axi_a_BREADY) begin
            m_axi_a_BREADY <= 1'b0;
            state <= ST_DONE;
          end
        end
        ST_DONE: begin
          interrupt <= 1'b1;
          state <= ST_DONE;
        end
        default: begin
          state <= ST_IDLE;
        end
      endcase
    end
  end

endmodule
