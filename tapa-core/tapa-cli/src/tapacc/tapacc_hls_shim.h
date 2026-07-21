// tapacc HLS analysis shim.
//
// tapacc runs the tapa-llvm clang purely to extract the dataflow graph. That
// clang is not the Xilinx Vitis HLS clang, so two Vitis-only constructs in the
// HLS system headers fail to type-check even though neither affects the graph:
//
//   * the `__builtin_bit_*` ap_int intrinsics (a Xilinx clang extension), and
//   * `__fp16` passed or returned by value (storage-only in stock clang).
//
// This header is force-included (`-include`) into the tapacc stage ONLY. It
// stubs both constructs so semantic analysis succeeds. It is never included in
// the `tapa-cpp` flatten stage, and tapacc extracts each task's `code` from the
// original source spelling, so the stubs never reach Vitis HLS synthesis.
#ifndef TAPACC_HLS_SHIM_H_
#define TAPACC_HLS_SHIM_H_

// Map the storage-only `__fp16` to a trivially-constructible float-like stub so
// it can be passed and returned by value. A distinct type (not `float` or
// `unsigned short`) avoids colliding with real specializations keyed on those.
namespace __tapacc_shim {
struct half_stub {
  half_stub() = default;                // trivial: usable as a union member
  half_stub(long double) {}             // construct from any numeric literal
  operator float() const { return 0; }  // single conversion -> unambiguous
  template <class T>
  half_stub& operator+=(T) {
    return *this;
  }
  template <class T>
  half_stub& operator-=(T) {
    return *this;
  }
  template <class T>
  half_stub& operator*=(T) {
    return *this;
  }
  template <class T>
  half_stub& operator/=(T) {
    return *this;
  }
};
}  // namespace __tapacc_shim
#define __fp16 __tapacc_shim::half_stub

// Stub the Xilinx ap_int bit intrinsics this clang lacks. Variadic templates
// swallow any argument list; each return kind (bool/void) matches how the HLS
// headers consume that intrinsic.
template <class... A>
static inline bool __tapacc_shim_bool(A...) {
  return false;
}
template <class... A>
static inline void __tapacc_shim_void(A...) {}
#define __builtin_bit_select(...) (__tapacc_shim_bool(__VA_ARGS__))
#define __builtin_bit_part_select(...) (__tapacc_shim_void(__VA_ARGS__))
#define __builtin_bit_part_set(...) (__tapacc_shim_void(__VA_ARGS__))
#define __builtin_bit_and_reduce(...) (__tapacc_shim_bool(__VA_ARGS__))
#define __builtin_bit_or_reduce(...) (__tapacc_shim_bool(__VA_ARGS__))
#define __builtin_bit_xor_reduce(...) (__tapacc_shim_bool(__VA_ARGS__))
#define __builtin_bit_nand_reduce(...) (__tapacc_shim_bool(__VA_ARGS__))
#define __builtin_bit_nor_reduce(...) (__tapacc_shim_bool(__VA_ARGS__))
#define __builtin_bit_xnor_reduce(...) (__tapacc_shim_bool(__VA_ARGS__))
#define __builtin_bit_concat(...) (__tapacc_shim_void(__VA_ARGS__))

#endif  // TAPACC_HLS_SHIM_H_
