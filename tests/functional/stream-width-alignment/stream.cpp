#include "stream.h"

void Forward(tapa::istream<Wide>& wide_in, tapa::istream<Signed>& signed_in,
             tapa::istream<Small>& small_in, tapa::ostream<Wide>& wide_out,
             tapa::ostream<Signed>& signed_out,
             tapa::ostream<Small>& small_out) {
  for (int transaction = 0; transaction < kTransactions; ++transaction) {
    for (int i = 0; i < TransactionSize(transaction); ++i) {
      Wide wide;
      while (!wide_in.try_peek(wide)) {
      }
      wide_in.read();
      wide_out.write(wide);
      signed_out.write(signed_in.read());
      small_out.write(small_in.read());
    }
    wide_in.open();
    signed_in.open();
    small_in.open();
    wide_out.close();
    signed_out.close();
    small_out.close();
  }
}

void StreamWidthAlignment(tapa::istream<Wide>& wide_in,
                          tapa::istream<Signed>& signed_in,
                          tapa::istream<Small>& small_in,
                          tapa::ostream<Wide>& wide_out,
                          tapa::ostream<Signed>& signed_out,
                          tapa::ostream<Small>& small_out) {
  tapa::stream<Wide> wide("wide");
  tapa::stream<Signed> signed_values("signed_values");
  tapa::stream<Small> small("small");
  tapa::task()
      .invoke(Forward, wide_in, signed_in, small_in, wide, signed_values, small)
      .invoke(Forward, wide, signed_values, small, wide_out, signed_out,
              small_out);
}
