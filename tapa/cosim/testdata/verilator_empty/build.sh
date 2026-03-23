#!/bin/bash
set -e
cd "$(dirname "$0")"



/verilator/bin/verilator --cc --top-module top \
  -Wno-fatal -Wno-PINMISSING -Wno-WIDTH -Wno-UNUSEDSIGNAL -Wno-UNDRIVEN -Wno-UNOPTFLAT -Wno-STMTDLY -Wno-CASEINCOMPLETE -Wno-SYMRSVDWORD -Wno-COMBDLY -Wno-TIMESCALEMOD -Wno-MULTIDRIVEN \
  --no-timing \
  --exe tb.cpp dpi_support.cpp \
  rtl/*.v 2>&1

make -C obj_dir -f Vtop.mk Vtop \
  -j$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4) 2>&1
