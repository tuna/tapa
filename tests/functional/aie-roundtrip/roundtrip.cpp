#include "roundtrip.h"

void Load(tapa::mmap<vec4i> input, int n, tapa::ostream<vec4i>& output) {
  for (int i = 0; i < n; ++i) {
    output.write(input[i]);
  }
}

[[tapa::target("aie")]]
void AddOne(tapa::istream<vec4i>& input, tapa::ostream<vec4i>& output, int n) {
  [[tapa::pipeline]] for (int i = 0; i < n; ++i) {
    output.write(input.read() + 1);
  }
}

void Double(tapa::istream<vec4i>& input, tapa::ostream<vec4i>& output, int n) {
  [[tapa::pipeline]] for (int i = 0; i < n; ++i) {
    output.write(input.read() * 2);
  }
}

[[tapa::target("aie")]]
void AddThree(tapa::istream<vec4i>& input, tapa::ostream<vec4i>& output,
              int n) {
  [[tapa::pipeline]] for (int i = 0; i < n; ++i) {
    output.write(input.read() + 3);
  }
}

void Store(tapa::istream<vec4i>& input, tapa::mmap<vec4i> output, int n) {
  for (int i = 0; i < n; ++i) {
    output[i] = input.read();
  }
}

void RoundTrip(tapa::mmap<vec4i> input, tapa::mmap<vec4i> output, int n) {
  tapa::stream<vec4i> input_q("input_q");
  tapa::stream<vec4i> plus_one_q("plus_one_q");
  tapa::stream<vec4i> doubled_q("doubled_q");
  tapa::stream<vec4i> output_q("output_q");

  tapa::task()
      .invoke(Load, input, n, input_q)
      .invoke(AddOne, input_q, plus_one_q, n)
      .invoke(Double, plus_one_q, doubled_q, n)
      .invoke(AddThree, doubled_q, output_q, n)
      .invoke(Store, output_q, output, n);
}
