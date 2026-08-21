#pragma once

// `tapa/base/axis.h` builds its signals out of tapa::u<W>, which here is the
// vendor ap_uint<W>, so the integer layer must come first; `widthof` comes
// from util.h.
#include "tapa/xilinx/hls/int.h"
#include "tapa/xilinx/hls/util.h"

#include "tapa/base/axis.h"
