#include <cstdint>

#include <tapa.h>

// Two Swap tasks trade elements: each sends its own to the other, then
// receives the other's. Without the fence, HLS may schedule the receive in
// the same cycle as the send, or ahead of it, and both tasks then wait for a
// send that neither issues.
void Swap(tapa::istream<float>& in, tapa::ostream<float>& to_peer,
          tapa::istream<float>& from_peer, tapa::ostream<float>& out,
          uint64_t n) {
  [[tapa::pipeline]] for (uint64_t i = 0; i < n; ++i) {
    const float mine = in.read();
    to_peer.write(mine);
    tapa::fence();
    out.write(mine + from_peer.read());
  }
}

void Mmap2Stream(tapa::mmap<const float> mmap, uint64_t n,
                 tapa::ostream<float>& stream) {
  for (uint64_t i = 0; i < n; ++i) stream << mmap[i];
}

void Stream2Mmap(tapa::istream<float>& stream, tapa::mmap<float> mmap,
                 uint64_t n) {
  for (uint64_t i = 0; i < n; ++i) stream >> mmap[i];
}

void Drain(tapa::istream<float>& stream, uint64_t n) {
  for (uint64_t i = 0; i < n; ++i) stream.read();
}

void VecAdd(tapa::mmap<const float> a, tapa::mmap<const float> b,
            tapa::mmap<float> c, uint64_t n) {
  tapa::stream<float> a_q("a");
  tapa::stream<float> b_q("b");
  tapa::stream<float> a_to_b("a_to_b");
  tapa::stream<float> b_to_a("b_to_a");
  tapa::stream<float> c_q("c");
  tapa::stream<float> d_q("d");

  tapa::task()
      .invoke(Mmap2Stream, a, n, a_q)
      .invoke(Mmap2Stream, b, n, b_q)
      .invoke(Swap, a_q, a_to_b, b_to_a, c_q, n)
      .invoke(Swap, b_q, b_to_a, a_to_b, d_q, n)
      .invoke(Stream2Mmap, c_q, c, n)
      .invoke(Drain, d_q, n);
}
