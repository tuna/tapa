`default_nettype none

(* keep_hierarchy = "yes" *)
module tapa_control_pipeline #(
  parameter WIDTH = 1,
  parameter BODY_LEVEL = 0
) (
  input  wire             clk,
  input  wire [WIDTH-1:0] in_data,
  output wire [WIDTH-1:0] out_data
);
  wire [WIDTH-1:0] body_data [0:BODY_LEVEL];

  (* keep_hierarchy = "yes" *)
  tapa_control_pipeline_stage #(
    .WIDTH(WIDTH)
  ) TAPA_HS_HEAD (
    .clk(clk),
    .in_data(in_data),
    .out_data(body_data[0])
  );

  genvar i;
  generate
    for (i = 0; i < BODY_LEVEL; i = i + 1) begin : TAPA_HS_BODY
      (* keep_hierarchy = "yes" *)
      tapa_control_pipeline_stage #(
        .WIDTH(WIDTH)
      ) TAPA_HS_BODY_REG (
        .clk(clk),
        .in_data(body_data[i]),
        .out_data(body_data[i+1])
      );
    end
  endgenerate

  (* keep_hierarchy = "yes" *)
  tapa_control_pipeline_stage #(
    .WIDTH(WIDTH)
  ) TAPA_HS_TAIL (
    .clk(clk),
    .in_data(body_data[BODY_LEVEL]),
    .out_data(out_data)
  );
endmodule

module tapa_control_pipeline_stage #(
  parameter WIDTH = 1
) (
  input  wire             clk,
  input  wire [WIDTH-1:0] in_data,
  output wire [WIDTH-1:0] out_data
);
  (* keep = "true" *) reg [WIDTH-1:0] data_reg = {WIDTH{1'b0}};

  always @(posedge clk) begin
    data_reg <= in_data;
  end

  assign out_data = data_reg;
endmodule

(* keep_hierarchy = "yes" *)
module tapa_global_controller #(
  parameter FLUSH_CYCLES = 0,
  parameter FLUSH_WIDTH = (FLUSH_CYCLES < 2) ? 1 : $clog2(FLUSH_CYCLES + 1)
) (
  input  wire ap_clk,
  input  wire ap_rst_n,
  input  wire ap_start,
  input  wire children_done,
  input  wire children_clear,
  output wire launch_start,
  output wire launch_release,
  output wire fabric_reset_n,
  output wire ap_done,
  output wire ap_ready,
  output wire ap_idle
);
  localparam [1:0] STATE_IDLE = 2'b00;
  localparam [1:0] STATE_RUNNING = 2'b01;
  localparam [1:0] STATE_RELEASING = 2'b11;
  localparam [1:0] STATE_DONE = 2'b10;

  reg [1:0] state = STATE_IDLE;
  reg [FLUSH_WIDTH-1:0] flush_count = FLUSH_CYCLES;
  reg pending_start = 1'b0;
  reg start_armed = 1'b1;
  wire flush_done;
  wire accept_ready;
  wire accept_start;

  assign flush_done = (flush_count == {FLUSH_WIDTH{1'b0}});
  assign accept_ready = flush_done & children_clear;
  assign accept_start = (ap_start | pending_start) & accept_ready &
                        (state == STATE_IDLE);
  assign launch_start = accept_start;
  assign launch_release = (state == STATE_RELEASING);
  assign fabric_reset_n = ap_rst_n & flush_done;
  assign ap_done = (state == STATE_DONE);
  assign ap_ready = (state == STATE_DONE);
  assign ap_idle = (state == STATE_IDLE);

  always @(posedge ap_clk) begin
    if (!ap_rst_n) begin
      state <= STATE_IDLE;
      flush_count <= FLUSH_CYCLES;
      pending_start <= 1'b0;
      start_armed <= 1'b1;
    end else begin
      // A level that remains high from the accepted request is not a new
      // request. Once it falls, remember one later reassertion even while the
      // current invocation is still running.
      if (!ap_start) begin
        start_armed <= 1'b1;
      end else if (start_armed) begin
        pending_start <= 1'b1;
        start_armed <= 1'b0;
      end

      if (!flush_done) begin
        flush_count <= flush_count - 1'b1;
      end else begin
        case (state)
          STATE_IDLE: begin
            if (accept_start) begin
              state <= STATE_RUNNING;
              pending_start <= 1'b0;
              start_armed <= ~ap_start;
            end
          end
          STATE_RUNNING: begin
            if (children_done) begin
              state <= STATE_RELEASING;
            end
          end
          STATE_RELEASING: begin
            if (children_clear) begin
              state <= STATE_DONE;
            end
          end
          STATE_DONE: state <= STATE_IDLE;
          default: state <= STATE_IDLE;
        endcase
      end
    end
  end
endmodule

(* keep_hierarchy = "yes" *)
module tapa_local_controller #(
  parameter AUTORUN = 0
) (
  input  wire ap_clk,
  input  wire reset_n,
  input  wire launch_start,
  input  wire launch_release,
  input  wire child_done,
  input  wire child_ready,
  input  wire child_idle,
  output wire child_start,
  output wire completion
);
  localparam [1:0] STATE_IDLE = 2'b00;
  localparam [1:0] STATE_RUNNING = 2'b01;
  localparam [1:0] STATE_WAITING = 2'b11;
  localparam [1:0] STATE_DONE = 2'b10;

  generate
    if (AUTORUN != 0) begin : TAPA_AUTORUN
      reg start_latched = 1'b0;

      always @(posedge ap_clk) begin
        if (!reset_n) begin
          start_latched <= 1'b0;
        end else if (launch_start) begin
          start_latched <= 1'b1;
        end
      end

      assign child_start = start_latched;
      assign completion = 1'b0;
    end else begin : TAPA_NORMAL
      reg [1:0] state = STATE_IDLE;

      always @(posedge ap_clk) begin
        if (!reset_n) begin
          state <= STATE_IDLE;
        end else begin
          case (state)
            STATE_IDLE: begin
              if (launch_start) begin
                state <= STATE_RUNNING;
              end
            end
            STATE_RUNNING: begin
              if (child_ready && child_done) begin
                state <= STATE_DONE;
              end else if (child_ready) begin
                state <= STATE_WAITING;
              end
            end
            STATE_WAITING: begin
              if (child_done) begin
                state <= STATE_DONE;
              end
            end
            STATE_DONE: begin
              if (launch_release) begin
                state <= STATE_IDLE;
              end
            end
            default: state <= STATE_IDLE;
          endcase
        end
      end

      assign child_start = (state == STATE_RUNNING);
      assign completion = (state == STATE_DONE);
    end
  endgenerate

  wire unused_child_idle = child_idle;
endmodule

`default_nettype wire
