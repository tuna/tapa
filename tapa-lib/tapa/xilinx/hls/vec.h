// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

// `tapa/base/vec.h` uses CHECK_GE/CHECK_LE, so the target's logging must come
// first; `widthof` comes from util.h. The HLS pragmas inside the shared bodies
// are switched on by `TAPA_TARGET_XILINX_HLS_`, which the synthesis flow
// defines on the command line.
#include "tapa/xilinx/hls/logging.h"
#include "tapa/xilinx/hls/util.h"

#include "tapa/base/vec.h"
