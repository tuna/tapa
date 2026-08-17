// tapa::u/i for the Xilinx HLS target: pure aliases of the vendor types, so
// the emitted source keeps the tapa spelling and resolves to ap_uint/ap_int
// here. No compiler rewrite is involved.

#pragma once

#include <ap_int.h>

namespace tapa {

template <int W>
using u = ap_uint<W>;
template <int W>
using i = ap_int<W>;

}  // namespace tapa
