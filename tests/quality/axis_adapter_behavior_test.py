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
    adapter = Path("tapa/assets/verilog/axis_adapter.v").resolve()
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
                str(adapter),
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
def test_axis_adapter_preserves_fwft_and_two_beat_buffering() -> None:
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

            // stream_to_axis keeps the old depth-2 FWFT contract.
            @(negedge clk);
            s_stream_din = 9'h155;
            s_stream_write = 1'b1;
            m_axis_tready = 1'b0;
            #0.1;
            check(m_axis_tvalid === 1'b1, "stream_to_axis fwft");
            check(
                {m_axis_tlast, m_axis_tdata} === 9'h155,
                "stream_to_axis fwft payload"
            );
            check(s_stream_full_n === 1'b1, "stream_to_axis second slot open");

            @(posedge clk);
            @(negedge clk);
            s_stream_din = 9'h0AA;
            s_stream_write = 1'b1;
            #0.1;
            check(s_stream_full_n === 1'b1, "stream_to_axis second slot open");

            @(posedge clk);
            @(negedge clk);
            s_stream_write = 1'b0;
            #0.1;
            check(s_stream_full_n === 1'b0, "stream_to_axis full after two beats");
            check(
                {m_axis_tlast, m_axis_tdata} === 9'h155,
                "stream_to_axis stalled head changed"
            );

            m_axis_tready = 1'b1;
            @(posedge clk);
            @(negedge clk);
            #0.1;
            check(
                {m_axis_tlast, m_axis_tdata} === 9'h0AA,
                "stream_to_axis order after drain"
            );

            @(posedge clk);
            @(negedge clk);
            #0.1;
            check(m_axis_tvalid === 1'b0, "stream_to_axis drained");

            // axis_to_stream also preserves FWFT plus one skid beat.
            s_axis_tdata = 8'h11;
            s_axis_tlast = 1'b1;
            s_axis_tvalid = 1'b1;
            m_stream_read = 1'b0;
            #0.1;
            check(m_stream_empty_n === 1'b1, "axis_to_stream fwft");
            check(m_stream_dout === 9'h111, "axis_to_stream fwft payload");
            check(s_axis_tready === 1'b1, "axis_to_stream second slot open");

            @(posedge clk);
            @(negedge clk);
            s_axis_tdata = 8'h22;
            s_axis_tlast = 1'b0;
            s_axis_tvalid = 1'b1;
            #0.1;
            check(s_axis_tready === 1'b1, "axis_to_stream second slot open");

            @(posedge clk);
            @(negedge clk);
            s_axis_tdata = 8'h33;
            s_axis_tlast = 1'b0;
            s_axis_tvalid = 1'b1;
            #0.1;
            check(s_axis_tready === 1'b0, "axis_to_stream backpressure");
            check(m_stream_dout === 9'h111, "axis_to_stream stalled head changed");

            m_stream_read = 1'b1;
            #0.1;
            check(s_axis_tready === 1'b1, "axis_to_stream reopens");
            s_axis_tvalid = 1'b0;
            @(posedge clk);
            @(negedge clk);
            #0.1;
            check(m_stream_dout === 9'h022, "axis_to_stream order after read");

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
