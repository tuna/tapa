"""Behavioral smoke tests for the explicit AXIS adapter RTL."""

from __future__ import annotations

import shutil
import subprocess
import tempfile
import textwrap
from pathlib import Path

import pytest

VERILATOR = shutil.which("verilator")


def _run_verilator_testbench(tb_source: str) -> None:
    assert VERILATOR is not None
    rtl_files = [
        Path("tapa/assets/verilog/axis_adapter.v").resolve(),
        Path("tapa/assets/verilog/fifo.v").resolve(),
        Path("tapa/assets/verilog/fifo_fwd.v").resolve(),
        Path("tapa/assets/verilog/fifo_srl.v").resolve(),
        Path("tapa/assets/verilog/fifo_bram.v").resolve(),
    ]
    with tempfile.TemporaryDirectory(prefix="axis-adapter-") as temp_dir:
        temp = Path(temp_dir)
        tb = temp / "tb.sv"
        tb.write_text(tb_source, encoding="utf-8")
        subprocess.run(
            [
                VERILATOR,
                "--binary",
                "--assert",
                "--sv",
                "--timing",
                "-Wno-fatal",
                *map(str, rtl_files),
                str(tb),
                "--top-module",
                "tb",
            ],
            cwd=temp,
            check=True,
            capture_output=True,
            text=True,
        )
        subprocess.run(
            [str(temp / "obj_dir" / "Vtb")],
            cwd=temp,
            check=True,
            capture_output=True,
            text=True,
        )


@pytest.mark.skipif(VERILATOR is None, reason="verilator not available")
def test_axis_adapter_registers_then_drains_single_beats() -> None:
    tb = textwrap.dedent(
        r"""
        `timescale 1ns/1ps
        module tb;
          reg clk = 1'b0;
          always #1 clk = ~clk;

          reg reset = 1'b1;

          reg  [7:0] s_axis_tdata = 8'h00;
          reg        s_axis_tvalid = 1'b0;
          wire       s_axis_tready;
          reg        s_axis_tlast = 1'b0;
          wire [8:0] m_stream_dout;
          wire       m_stream_empty_n;
          reg        m_stream_read = 1'b0;

          reg  [8:0] s_stream_din = 9'h000;
          wire       s_stream_full_n;
          reg        s_stream_write = 1'b0;
          wire [7:0] m_axis_tdata;
          wire       m_axis_tvalid;
          reg        m_axis_tready = 1'b0;
          wire       m_axis_tlast;

          axis_to_stream_adapter #(.DATA_WIDTH(8)) axis_to_stream (
            .clk(clk),
            .reset(reset),
            .s_axis_tdata(s_axis_tdata),
            .s_axis_tvalid(s_axis_tvalid),
            .s_axis_tready(s_axis_tready),
            .s_axis_tlast(s_axis_tlast),
            .m_stream_dout(m_stream_dout),
            .m_stream_empty_n(m_stream_empty_n),
            .m_stream_read(m_stream_read)
          );

          stream_to_axis_adapter #(.DATA_WIDTH(8)) stream_to_axis (
            .clk(clk),
            .reset(reset),
            .s_stream_din(s_stream_din),
            .s_stream_full_n(s_stream_full_n),
            .s_stream_write(s_stream_write),
            .m_axis_tdata(m_axis_tdata),
            .m_axis_tvalid(m_axis_tvalid),
            .m_axis_tready(m_axis_tready),
            .m_axis_tlast(m_axis_tlast)
          );

          task automatic check(input bit cond, input string msg);
            if (!cond) begin
              $display("FAIL: %s", msg);
              $fatal(1);
            end
          endtask

          initial begin
            repeat (2) @(posedge clk);
            reset = 1'b0;

            // stream_to_axis matches the old depth-2 FIFO wrapper behavior.
            @(negedge clk);
            s_stream_din = 9'h155;
            s_stream_write = 1'b1;
            m_axis_tready = 1'b0;
            #0.1;
            check(m_axis_tvalid === 1'b0, "stream_to_axis is registered");
            @(posedge clk);
            @(negedge clk);
            #0.1;
            check(m_axis_tvalid === 1'b1, "stream_to_axis first beat visible");
            check(
                {m_axis_tlast, m_axis_tdata} === 9'h155,
                "stream_to_axis first beat payload"
            );
            s_stream_write = 1'b0;

            m_axis_tready = 1'b1;
            @(posedge clk);
            @(negedge clk);
            #0.1;
            check(m_axis_tvalid === 1'b0, "stream_to_axis drained");

            // axis_to_stream also matches the old depth-2 FIFO wrapper behavior.
            s_axis_tdata = 8'h11;
            s_axis_tlast = 1'b1;
            s_axis_tvalid = 1'b1;
            m_stream_read = 1'b0;
            #0.1;
            check(m_stream_empty_n === 1'b0, "axis_to_stream is registered");

            @(posedge clk);
            @(negedge clk);
            #0.1;
            check(m_stream_empty_n === 1'b1, "axis_to_stream first beat visible");
            check(m_stream_dout === 9'h111, "axis_to_stream first beat payload");

            m_stream_read = 1'b1;
            s_axis_tvalid = 1'b0;
            @(posedge clk);
            @(negedge clk);
            #0.1;
            check(m_stream_empty_n === 1'b0, "axis_to_stream drained");

            $finish;
          end
        endmodule
        """
    )

    _run_verilator_testbench(tb)


@pytest.mark.skipif(VERILATOR is None, reason="verilator not available")
def test_axis_adapters_preserve_order_under_sustained_backpressure() -> None:
    tb = textwrap.dedent(
        r"""
        `timescale 1ns/1ps
        module tb;
          localparam int COUNT = 16;

          reg clk = 1'b0;
          always #1 clk = ~clk;

          reg reset = 1'b1;

          reg  [7:0] s_axis_tdata = 8'h00;
          reg        s_axis_tvalid = 1'b0;
          wire       s_axis_tready;
          reg        s_axis_tlast = 1'b0;
          wire [8:0] m_stream_dout;
          wire       m_stream_empty_n;
          reg        m_stream_read = 1'b0;

          reg  [8:0] s_stream_din = 9'h000;
          wire       s_stream_full_n;
          reg        s_stream_write = 1'b0;
          wire [7:0] m_axis_tdata;
          wire       m_axis_tvalid;
          reg        m_axis_tready = 1'b0;
          wire       m_axis_tlast;

          integer cycle = 0;
          integer axis_send_idx = 0;
          integer axis_recv_idx = 0;
          integer stream_send_idx = 0;
          integer stream_recv_idx = 0;

          axis_to_stream_adapter #(.DATA_WIDTH(8)) axis_to_stream (
            .clk(clk),
            .reset(reset),
            .s_axis_tdata(s_axis_tdata),
            .s_axis_tvalid(s_axis_tvalid),
            .s_axis_tready(s_axis_tready),
            .s_axis_tlast(s_axis_tlast),
            .m_stream_dout(m_stream_dout),
            .m_stream_empty_n(m_stream_empty_n),
            .m_stream_read(m_stream_read)
          );

          stream_to_axis_adapter #(.DATA_WIDTH(8)) stream_to_axis (
            .clk(clk),
            .reset(reset),
            .s_stream_din(s_stream_din),
            .s_stream_full_n(s_stream_full_n),
            .s_stream_write(s_stream_write),
            .m_axis_tdata(m_axis_tdata),
            .m_axis_tvalid(m_axis_tvalid),
            .m_axis_tready(m_axis_tready),
            .m_axis_tlast(m_axis_tlast)
          );

          task automatic check(input bit cond, input string msg);
            if (!cond) begin
              $display("FAIL: %s", msg);
              $fatal(1);
            end
          endtask

          function automatic [8:0] axis_payload(input integer idx);
            axis_payload = {idx[0], idx[7:0]};
          endfunction

          function automatic [8:0] stream_payload(input integer idx);
            stream_payload = {~idx[0], (idx * 7 + 8'h31) & 8'hff};
          endfunction

          always @(posedge clk) begin
            if (reset) begin
              cycle <= 0;
            end else begin
              cycle <= cycle + 1;

              if (s_axis_tvalid && s_axis_tready) begin
                axis_send_idx <= axis_send_idx + 1;
              end

              if (m_stream_empty_n && m_stream_read) begin
                check(
                    m_stream_dout === axis_payload(axis_recv_idx),
                    $sformatf(
                        "axis_to_stream out of order at %0d: got %0h expected %0h",
                        axis_recv_idx,
                        m_stream_dout,
                        axis_payload(axis_recv_idx)
                    )
                );
                axis_recv_idx <= axis_recv_idx + 1;
              end

              if (m_axis_tvalid && m_axis_tready) begin
                check(
                    {m_axis_tlast, m_axis_tdata} === stream_payload(stream_recv_idx),
                    $sformatf(
                        "stream_to_axis out of order at %0d: got %0h expected %0h",
                        stream_recv_idx,
                        {m_axis_tlast, m_axis_tdata},
                        stream_payload(stream_recv_idx)
                    )
                );
                stream_recv_idx <= stream_recv_idx + 1;
              end

              if (axis_recv_idx == COUNT && stream_recv_idx == COUNT) begin
                $finish;
              end

              if (cycle > 80) begin
                $display("FAIL: timeout");
                $fatal(1);
              end
            end
          end

          always @(*) begin
            if (reset) begin
              s_axis_tvalid = 1'b0;
              s_axis_tdata = 8'h00;
              s_axis_tlast = 1'b0;
              m_stream_read = 1'b0;
              s_stream_write = 1'b0;
              s_stream_din = 9'h000;
              m_axis_tready = 1'b0;
            end else begin
              s_axis_tvalid = (axis_send_idx < COUNT);
              s_axis_tdata = axis_payload(axis_send_idx)[7:0];
              s_axis_tlast = axis_payload(axis_send_idx)[8];
              m_stream_read = (cycle % 3) != 1;

              s_stream_write = (stream_send_idx < COUNT) && s_stream_full_n;
              s_stream_din = stream_payload(stream_send_idx);
              m_axis_tready = (cycle % 4) < 2;
            end
          end

          always @(posedge clk) begin
            if (!reset && s_stream_write) begin
              stream_send_idx <= stream_send_idx + 1;
            end
          end

          initial begin
            repeat (2) @(posedge clk);
            reset = 1'b0;
          end
        endmodule
        """
    )

    _run_verilator_testbench(tb)


@pytest.mark.skipif(VERILATOR is None, reason="verilator not available")
def test_axis_adapters_match_previous_fifo_wrapper_behavior() -> None:
    tb = textwrap.dedent(
        r"""
        `timescale 1ns/1ps
        module tb;
          localparam int DATA_WIDTH = 8;

          reg clk = 1'b0;
          always #1 clk = ~clk;

          reg reset = 1'b1;

          reg  [DATA_WIDTH-1:0] s_axis_tdata = '0;
          reg                   s_axis_tvalid = 1'b0;
          wire                  s_axis_tready_dut;
          wire                  s_axis_tready_ref;
          reg                   s_axis_tlast = 1'b0;

          wire [DATA_WIDTH:0]   m_stream_dout_dut;
          wire                  m_stream_empty_n_dut;
          wire [DATA_WIDTH:0]   m_stream_dout_ref;
          wire                  m_stream_empty_n_ref;
          reg                   m_stream_read = 1'b0;

          reg  [DATA_WIDTH:0]   s_stream_din = '0;
          wire                  s_stream_full_n_dut;
          wire                  s_stream_full_n_ref;
          reg                   s_stream_write = 1'b0;

          wire [DATA_WIDTH-1:0] m_axis_tdata_dut;
          wire                  m_axis_tvalid_dut;
          reg                   m_axis_tready = 1'b0;
          wire                  m_axis_tlast_dut;

          wire [DATA_WIDTH-1:0] m_axis_tdata_ref;
          wire                  m_axis_tvalid_ref;
          wire                  m_axis_tlast_ref;

          integer cycle = 0;
          integer axis_send_idx = 0;
          integer stream_send_idx = 0;

          axis_to_stream_adapter #(.DATA_WIDTH(DATA_WIDTH)) axis_to_stream_dut (
            .clk(clk),
            .reset(reset),
            .s_axis_tdata(s_axis_tdata),
            .s_axis_tvalid(s_axis_tvalid),
            .s_axis_tready(s_axis_tready_dut),
            .s_axis_tlast(s_axis_tlast),
            .m_stream_dout(m_stream_dout_dut),
            .m_stream_empty_n(m_stream_empty_n_dut),
            .m_stream_read(m_stream_read)
          );

          fifo #(
            .DATA_WIDTH(DATA_WIDTH + 1),
            .ADDR_WIDTH(1),
            .DEPTH(2)
          ) axis_to_stream_ref (
            .clk(clk),
            .reset(reset),
            .if_full_n(s_axis_tready_ref),
            .if_write_ce(1'b1),
            .if_write(s_axis_tvalid),
            .if_din({s_axis_tlast, s_axis_tdata}),
            .if_empty_n(m_stream_empty_n_ref),
            .if_read_ce(1'b1),
            .if_read(m_stream_read),
            .if_dout(m_stream_dout_ref)
          );

          stream_to_axis_adapter #(.DATA_WIDTH(DATA_WIDTH)) stream_to_axis_dut (
            .clk(clk),
            .reset(reset),
            .s_stream_din(s_stream_din),
            .s_stream_full_n(s_stream_full_n_dut),
            .s_stream_write(s_stream_write),
            .m_axis_tdata(m_axis_tdata_dut),
            .m_axis_tvalid(m_axis_tvalid_dut),
            .m_axis_tready(m_axis_tready),
            .m_axis_tlast(m_axis_tlast_dut)
          );

          wire [DATA_WIDTH:0] ref_axis_payload;
          fifo #(
            .DATA_WIDTH(DATA_WIDTH + 1),
            .ADDR_WIDTH(1),
            .DEPTH(2)
          ) stream_to_axis_ref (
            .clk(clk),
            .reset(reset),
            .if_full_n(s_stream_full_n_ref),
            .if_write_ce(1'b1),
            .if_write(s_stream_write),
            .if_din(s_stream_din),
            .if_empty_n(m_axis_tvalid_ref),
            .if_read_ce(1'b1),
            .if_read(m_axis_tready),
            .if_dout(ref_axis_payload)
          );
          assign {m_axis_tlast_ref, m_axis_tdata_ref} = ref_axis_payload;

          task automatic check(input bit cond, input string msg);
            if (!cond) begin
              $display("FAIL: %s", msg);
              $fatal(1);
            end
          endtask

          function automatic [DATA_WIDTH:0] axis_payload(input integer idx);
            axis_payload = {idx[0], idx[7:0]};
          endfunction

          function automatic [DATA_WIDTH:0] stream_payload(input integer idx);
            stream_payload = {~idx[0], (idx * 5 + 8'h12) & 8'hff};
          endfunction

          always @(posedge clk) begin
            if (reset) begin
              cycle <= 0;
            end else begin
              cycle <= cycle + 1;

              check(
                  s_axis_tready_dut === s_axis_tready_ref,
                  $sformatf("axis_to_stream tready mismatch at cycle %0d", cycle)
              );
              check(
                  m_stream_empty_n_dut === m_stream_empty_n_ref,
                  $sformatf("axis_to_stream empty_n mismatch at cycle %0d", cycle)
              );
              if (m_stream_empty_n_ref) begin
                check(
                    m_stream_dout_dut === m_stream_dout_ref,
                    $sformatf("axis_to_stream dout mismatch at cycle %0d", cycle)
                );
              end

              check(
                  s_stream_full_n_dut === s_stream_full_n_ref,
                  $sformatf("stream_to_axis full_n mismatch at cycle %0d", cycle)
              );
              check(
                  m_axis_tvalid_dut === m_axis_tvalid_ref,
                  $sformatf("stream_to_axis tvalid mismatch at cycle %0d", cycle)
              );
              if (m_axis_tvalid_ref) begin
                check(
                    {m_axis_tlast_dut, m_axis_tdata_dut} ===
                        {m_axis_tlast_ref, m_axis_tdata_ref},
                    $sformatf("stream_to_axis payload mismatch at cycle %0d", cycle)
                );
              end

              if (s_axis_tvalid && s_axis_tready_ref) begin
                axis_send_idx <= axis_send_idx + 1;
              end
              if (s_stream_write && s_stream_full_n_ref) begin
                stream_send_idx <= stream_send_idx + 1;
              end

              if (cycle > 40) $finish;
            end
          end

          always @(*) begin
            if (reset) begin
              s_axis_tvalid = 1'b0;
              s_axis_tdata = '0;
              s_axis_tlast = 1'b0;
              m_stream_read = 1'b0;
              s_stream_write = 1'b0;
              s_stream_din = '0;
              m_axis_tready = 1'b0;
            end else begin
              s_axis_tvalid = axis_send_idx < 12;
              s_axis_tdata = axis_payload(axis_send_idx)[DATA_WIDTH-1:0];
              s_axis_tlast = axis_payload(axis_send_idx)[DATA_WIDTH];
              m_stream_read = (cycle % 3) != 0;

              s_stream_write = (stream_send_idx < 12) && s_stream_full_n_ref;
              s_stream_din = stream_payload(stream_send_idx);
              m_axis_tready = (cycle % 4) < 2;
            end
          end

          initial begin
            repeat (2) @(posedge clk);
            reset = 1'b0;
          end
        endmodule
        """
    )

    _run_verilator_testbench(tb)
