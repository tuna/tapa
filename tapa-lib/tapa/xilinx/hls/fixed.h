#pragma once

// tapa::fixed/ufixed for the Xilinx HLS target: aliases of the vendor types,
// so the emitted source keeps the tapa spelling and resolves to
// ap_fixed/ap_ufixed here. Fixed-point arithmetic is something the HLS
// compiler implements natively; the portable implementation exists so
// software simulation agrees with it, not to be synthesized in its place.

#include <ap_fixed.h>

#include "tapa/base/fixed.h"  // the q_mode/o_mode enums, declared for both

namespace tapa {

template <int W, int I, q_mode Q = q_mode::trn, o_mode O = o_mode::wrap,
          int N = 0>
using fixed = ap_fixed<W, I, static_cast<ap_q_mode>(Q),
                       static_cast<ap_o_mode>(O), N>;

template <int W, int I, q_mode Q = q_mode::trn, o_mode O = o_mode::wrap,
          int N = 0>
using ufixed = ap_ufixed<W, I, static_cast<ap_q_mode>(Q),
                         static_cast<ap_o_mode>(O), N>;

}  // namespace tapa
