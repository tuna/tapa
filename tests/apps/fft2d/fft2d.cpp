#include "fft2d.h"

// The row pass transforms each row along its columns, which lie across the
// lanes of `kGroups` vectors: the stages that pair elements at least
// `kLanes` apart pair whole vectors, and the narrower ones pair lanes,
// brought together by interleave_unzip and put back by interleave_zip. The
// column pass transforms `kLanes` columns at once, one per lane: it reads one
// vector per row of the row pass's frame, so every butterfly is a plain
// vector operation and nothing is transposed. Without frame ports the column
// pass would first copy its whole frame into a local array to reach a column.

namespace {

struct cvec {
  vec re;
  vec im;
};

// W^k = exp(-2 pi i k / 32) = kCos[k] - i kSin[k], for k < 16.
constexpr float kCos[16] = {
    1.000000000f,  0.980785280f,  0.923879533f,  0.831469612f,
    0.707106781f,  0.555570233f,  0.382683432f,  0.195090322f,
    0.000000000f,  -0.195090322f, -0.382683432f, -0.555570233f,
    -0.707106781f, -0.831469612f, -0.923879533f, -0.980785280f};
constexpr float kSin[16] = {
    0.000000000f, 0.195090322f, 0.382683432f, 0.555570233f,
    0.707106781f, 0.831469612f, 0.923879533f, 0.980785280f,
    1.000000000f, 0.980785280f, 0.923879533f, 0.831469612f,
    0.707106781f, 0.555570233f, 0.382683432f, 0.195090322f};

// A decimation-in-frequency butterfly: a + b, and (a - b) times the twiddle.
template <typename W>
inline void Butterfly(cvec& a, cvec& b, const W& wr, const W& wi) {
  const vec dr = a.re - b.re;
  const vec di = a.im - b.im;
  a.re += b.re;
  a.im += b.im;
  vec re = dr * wr;
  re += di * wi;  // W = cos - i sin: wi holds sin
  vec im = di * wr;
  im -= dr * wi;
  b.re = re;
  b.im = im;
}

inline void Butterfly(cvec& a, cvec& b) {
  const cvec d = {a.re - b.re, a.im - b.im};
  a.re += b.re;
  a.im += b.im;
  b = d;
}

// The twiddles of the row pass, by lane. The stage pairing elements `d`
// apart within blocks of `2d` multiplies element `e` of a block by
// W^(e * 16 / d). A whole-vector stage `dv = d / kLanes` vectors apart keeps
// its `dv` twiddle vectors at [dv - 1, 2 dv - 1); a lane stage `d` apart,
// at [d].
struct RowTwiddles {
  vec cos_v[kGroups - 1], sin_v[kGroups - 1];
  vec cos_l[kLanes], sin_l[kLanes];
};

RowTwiddles MakeRowTwiddles() {
  RowTwiddles tw;
  for (int dv = 1; dv < kGroups; dv *= 2) {
    for (int j = 0; j < dv; ++j) {
      for (int lane = 0; lane < kLanes; ++lane) {
        const int k = (j * kLanes + lane) * (kEdge / 2) / (dv * kLanes);
        tw.cos_v[dv - 1 + j].set(lane, kCos[k]);
        tw.sin_v[dv - 1 + j].set(lane, kSin[k]);
      }
    }
  }
  for (int d = 1; d < kLanes; d *= 2) {
    for (int lane = 0; lane < kLanes; ++lane) {
      const int k = lane % d * (kEdge / 2) / d;
      tw.cos_l[d].set(lane, kCos[k]);
      tw.sin_l[d].set(lane, kSin[k]);
    }
  }
  return tw;
}

// The lane stages from `kStep` apart down to neighbours, on the `2 * kLanes`
// elements of `x` and `y`.
template <int kStep>
inline void LaneStages(cvec& x, cvec& y, const RowTwiddles& tw) {
  auto re = tapa::interleave_unzip<kStep>(x.re, y.re);
  auto im = tapa::interleave_unzip<kStep>(x.im, y.im);
  cvec a = {re.first, im.first};
  cvec b = {re.second, im.second};
  if constexpr (kStep == 1) {
    Butterfly(a, b);
  } else {
    Butterfly(a, b, tw.cos_l[kStep], tw.sin_l[kStep]);
  }
  re = tapa::interleave_zip<kStep>(a.re, b.re);
  im = tapa::interleave_zip<kStep>(a.im, b.im);
  x = {re.first, im.first};
  y = {re.second, im.second};
  if constexpr (kStep > 1) LaneStages<kStep / 2>(x, y, tw);
}

// The whole-vector stages from `kDv` vectors apart down to neighbours. Each
// stage has a constant trip count, which the AI Engine compiler needs to
// unroll it.
template <int kDv>
inline void VectorStages(cvec x[kGroups], const RowTwiddles& tw) {
  for (int s = 0; s < kGroups; s += 2 * kDv) {
    for (int j = 0; j < kDv; ++j) {
      Butterfly(x[s + j], x[s + j + kDv], tw.cos_v[kDv - 1 + j],
                tw.sin_v[kDv - 1 + j]);
    }
  }
  if constexpr (kDv > 1) VectorStages<kDv / 2>(x, tw);
}

inline void RowFft(cvec x[kGroups], const RowTwiddles& tw) {
  VectorStages<kGroups / 2>(x, tw);
  for (int p = 0; p < kGroups; p += 2) {
    LaneStages<kLanes / 2>(x[p], x[p + 1], tw);
  }
}

// The column pass's stages from `kD` vectors apart down to neighbours, each
// one pipelined loop over its 16 butterflies: one twiddle per butterfly,
// broadcast to every lane.
template <int kD>
inline void ColFft(cvec x[kEdge]) {
  [[tapa::pipeline(1)]] [[tapa::dependence(
      "x", "", "inter")]] for (int b = 0; b < kEdge / 2; ++b) {
    const int j = b % kD;
    const int s = b / kD * 2 * kD;
    const int k = j * (kEdge / 2) / kD;
    Butterfly(x[s + j], x[s + j + kD], kCos[k], kSin[k]);
  }
  if constexpr (kD > 1) ColFft<kD / 2>(x);
}

}  // namespace

void Load(tapa::mmap<const vec> src, tapa::ostream<vec>& out, int n) {
  [[tapa::pipeline(1)]] for (int i = 0; i < n * kFrame; ++i) {
    out.write(src[i]);
  }
}

void RowPass(tapa::iframes<vec, kFrame>& in, tapa::oframes<vec, kFrame>& out,
             int n) {
  const RowTwiddles tw = MakeRowTwiddles();
  for (int f = 0; f < n; ++f) {
    auto src = in.acquire();
    auto dst = out.acquire();
    [[tapa::pipeline(2 * kGroups)]] for (int r = 0; r < kEdge; ++r) {
      cvec x[kGroups];
      for (int h = 0; h < kGroups; ++h) {
        x[h] = {src[r * 2 * kGroups + h], src[r * 2 * kGroups + kGroups + h]};
      }
      RowFft(x, tw);
      for (int h = 0; h < kGroups; ++h) {
        dst[r * 2 * kGroups + h] = x[h].re;
        dst[r * 2 * kGroups + kGroups + h] = x[h].im;
      }
    }
  }
}

void ColPass(tapa::iframes<vec, kFrame>& in, tapa::oframes<vec, kFrame>& out,
             int n) {
  for (int f = 0; f < n; ++f) {
    auto src = in.acquire();
    auto dst = out.acquire();
    for (int h = 0; h < kGroups; ++h) {
      cvec x[kEdge];
      [[tapa::pipeline(1)]] for (int r = 0; r < kEdge; ++r) {
        x[r] = {src[r * 2 * kGroups + h], src[r * 2 * kGroups + kGroups + h]};
      }
      ColFft<kEdge / 2>(x);
      [[tapa::pipeline(1)]] for (int r = 0; r < kEdge; ++r) {
        dst[(h * kEdge + r) * 2] = x[r].re;
        dst[(h * kEdge + r) * 2 + 1] = x[r].im;
      }
    }
  }
}

void Store(tapa::istream<vec>& in, tapa::mmap<vec> dst, int n) {
  [[tapa::pipeline(1)]] for (int i = 0; i < n * kFrame; ++i) {
    dst[i] = in.read();
  }
}

void Fft2d(tapa::mmap<const vec> src, tapa::mmap<vec> dst, int n) {
  tapa::stream<vec, 2 * kFrame> rows("rows");
  tapa::stream<vec, 2 * kFrame> spectra("spectra");
  tapa::stream<vec, 2 * kFrame> cols("cols");
  tapa::task()
      .invoke(Load, src, rows, n)
      .invoke(RowPass, rows, spectra, n)
      .invoke(ColPass, spectra, cols, n)
      .invoke(Store, cols, dst, n);
}
