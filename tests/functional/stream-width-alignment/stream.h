#pragma once

#include <tapa.h>

using Wide = tapa::u<544>;
using Signed = tapa::i<17>;
using Small = tapa::u<7>;

constexpr int kTransactions = 3;
constexpr int TransactionSize(int transaction) {
  return transaction == 0 ? 0 : transaction * 4 + 1;
}

void StreamWidthAlignment(tapa::istream<Wide>& wide_in,
                          tapa::istream<Signed>& signed_in,
                          tapa::istream<Small>& small_in,
                          tapa::ostream<Wide>& wide_out,
                          tapa::ostream<Signed>& signed_out,
                          tapa::ostream<Small>& small_out);
