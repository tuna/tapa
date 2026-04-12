module manual_hls_top (
    input wire ap_clk,
    input wire ap_rst_n,
    input wire ap_start,
    output reg ap_done,
    output reg ap_ready,
    output reg ap_idle,

    input wire [63:0] a_offset,

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

    input wire [32:0] s_s_dout,
    input wire s_s_empty_n,
    output reg s_s_read
);

  localparam ST_IDLE = 0;
  localparam ST_AR = 1;
  localparam ST_R = 2;
  localparam ST_STREAM = 3;
  localparam ST_AW_W = 4;
  localparam ST_B = 5;
  localparam ST_DONE = 6;

  reg [2:0] state;
  reg [31:0] read_word;
  reg [31:0] stream_word;
  reg aw_sent;
  reg w_sent;

  always @(posedge ap_clk) begin
    if (!ap_rst_n) begin
      state <= ST_IDLE;
      ap_done <= 1'b0;
      ap_ready <= 1'b0;
      ap_idle <= 1'b1;

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
      s_s_read <= 1'b0;
      aw_sent <= 1'b0;
      w_sent <= 1'b0;
      read_word <= 32'd0;
      stream_word <= 32'd0;
    end else begin
      s_s_read <= 1'b0;
      case (state)
        ST_IDLE: begin
          ap_done <= 1'b0;
          ap_ready <= 1'b0;
          ap_idle <= 1'b1;
          m_axi_a_ARVALID <= 1'b0;
          m_axi_a_AWVALID <= 1'b0;
          m_axi_a_WVALID <= 1'b0;
          m_axi_a_BREADY <= 1'b0;
          m_axi_a_RREADY <= 1'b0;
          if (ap_start) begin
            ap_idle <= 1'b0;
            state <= ST_AR;
          end
        end
        ST_AR: begin
          m_axi_a_ARADDR <= a_offset;
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
            state <= ST_STREAM;
          end
        end
        ST_STREAM: begin
          if (s_s_empty_n) begin
            s_s_read <= 1'b1;
            stream_word <= s_s_dout[31:0];
            state <= ST_AW_W;
            aw_sent <= 1'b0;
            w_sent <= 1'b0;
          end
        end
        ST_AW_W: begin
          m_axi_a_AWADDR <= a_offset;
          m_axi_a_AWVALID <= !aw_sent;
          if (m_axi_a_AWVALID && m_axi_a_AWREADY) begin
            aw_sent <= 1'b1;
            m_axi_a_AWVALID <= 1'b0;
          end

          m_axi_a_WDATA <= read_word + stream_word;
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
          ap_done <= 1'b1;
          ap_ready <= 1'b1;
          ap_idle <= 1'b1;
          state <= ST_DONE;
        end
        default: begin
          state <= ST_IDLE;
        end
      endcase
    end
  end

endmodule
