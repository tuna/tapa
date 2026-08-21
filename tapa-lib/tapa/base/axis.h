// AXI4-Stream packet types: tapa::axis<T, WUser, WId, WDest>, the portable
// form of the vendor ap_axiu / ap_axis families.
//
// Unlike tapa::u/i, this is ONE definition for every target rather than a
// vendor alias on synthesis targets. The type is a struct whose members are
// tapa::u<W> -- which IS ap_uint<W> under the Xilinx target -- so the vendor
// sees member for member what ap_axiu declares, while software simulation
// needs no vendor header. Object size, alignment and every member offset
// match the vendor's, which is what mmap element stride and stream element
// size depend on: tapa/vendor_parity_test.cpp checks that against ap_axiu
// directly where the vendor headers are installed, and host/axis_test.cpp
// pins the same numbers where they are not.
//
// Scope: this replaces the ap_axiu / ap_axis *packet types*, which is how
// TAPA designs use them -- as struct payloads on tapa::stream. The vendor's
// per-signal `EnableSignals` bit field has no portable form here: TDATA,
// TKEEP, TSTRB and TLAST are always present, and TUSER/TID/TDEST are present
// exactly when their width is non-zero, which is what
// ap_axiu<W, WUser, WId, WDest> means.
//
// C++14 only: this header is compiled for Vitis HLS, which accepts no later
// standard, so a disabled signal is handled by overload rather than by
// `if constexpr`.

#pragma once

#include <climits>
#include <ostream>
#include <type_traits>

// The packet members are tapa::u<W>, so this header needs the integer
// layer, not just the layering comment that used to stand in for it
// (under the Xilinx target int.h is inert and the umbrella's hls/int.h
// alias applies).
#include "tapa/base/int.h"
#include "tapa/base/util.h"

namespace tapa {

namespace internal {

// A signal the packet does not carry. The vendor keeps a member for every
// disabled signal rather than dropping it, so the enabled members keep their
// offsets; matching that keeps sizeof() and every offset identical.
struct axis_disabled_signal {};

// Mirror of the vendor's bitwidth pair (ap_axi_sdata.h): its width_keep is
// bytewidth(data_type), and the ap_uint<W> partial specialization of its
// bitwidth decides between ceil(W/8) and sizeof(). That specialization is
// keyed on std::size_t while ap_uint's parameter is int, so whether it
// applies is the host compiler's call (Clang applies it, GCC does not) —
// and the vendor's width_keep changes with it. Specializing with the SAME
// kind mismatch here keeps width_keep equal to the vendor's under either
// compiler, because sizeof(tapa::u<W>) == sizeof(ap_uint<W>).
template <typename T>
struct axis_bitwidth {
  static constexpr int value = static_cast<int>(sizeof(T)) * CHAR_BIT;
};
template <std::size_t W>
struct axis_bitwidth<u<W>> {
  static constexpr int value = W;
};
template <std::size_t W>
struct axis_bitwidth<i<W>> {
  static constexpr int value = W;
};

template <bool kEnabled, int W>
struct axis_signal {
  using type = u<W>;
};

template <int W>
struct axis_signal<false, W> {
  using type = axis_disabled_signal;
};

// Accessing a signal the packet does not carry: assignment and comparison
// become no-ops, reads yield zero, and printing emits nothing. The
// alternative -- the vendor's run-time throw -- reports the mistake only if
// simulation happens to reach that line.
template <int W>
inline void axis_set(u<W>& lhs, const u<W>& rhs) {
  lhs = rhs;
}
template <int W>
inline void axis_set(axis_disabled_signal&, const u<W>&) {}

template <int W>
inline u<W> axis_get(const u<W>& value) {
  return value;
}
inline u<1> axis_get(const axis_disabled_signal&) { return 0; }

template <int W>
inline bool axis_eq(const u<W>& lhs, const u<W>& rhs) {
  return lhs == rhs;
}
inline bool axis_eq(const axis_disabled_signal&, const axis_disabled_signal&) {
  return true;
}

template <int W>
inline void axis_print(std::ostream& os, const char* name, const u<W>& value) {
  os << ", " << name << ": " << value;
}
inline void axis_print(std::ostream&, const char*,
                       const axis_disabled_signal&) {}

}  // namespace internal

/// An AXI4-Stream packet carrying a @p T payload.
///
/// @tparam T      Payload type of the TDATA signal.
/// @tparam WUser  Width of TUSER; 0 disables the signal.
/// @tparam WId    Width of TID; 0 disables the signal.
/// @tparam WDest  Width of TDEST; 0 disables the signal.
template <typename T, int WUser = 0, int WId = 0, int WDest = 0>
struct axis {
  static_assert(!std::is_void<T>::value,
                "tapa::axis needs a payload type; a packet with no TDATA has "
                "no portable form");
  static_assert(WUser >= 0 && WId >= 0 && WDest >= 0,
                "signal widths cannot be negative");

  static constexpr bool has_user = WUser > 0;
  static constexpr bool has_id = WId > 0;
  static constexpr bool has_dest = WDest > 0;
  // TDATA, TKEEP, TSTRB and TLAST are always present (see the header).
  static constexpr bool has_data = true;
  static constexpr bool has_keep = true;
  static constexpr bool has_strb = true;
  static constexpr bool has_last = true;

  static constexpr int width_data = widthof<T>();
  // TKEEP and TSTRB carry one bit per payload BYTE, as AXI4-Stream defines
  // them -- and as the vendor's bytewidth computes it, whichever way the
  // host compiler resolves its bitwidth specialization.
  static constexpr int width_keep =
      (internal::axis_bitwidth<T>::value + CHAR_BIT - 1) / CHAR_BIT;
  static constexpr int width_strb = width_keep;
  static constexpr int width_last = 1;
  static constexpr int width_user = has_user ? WUser : 1;
  static constexpr int width_id = has_id ? WId : 1;
  static constexpr int width_dest = has_dest ? WDest : 1;

  using data_type = T;
  using keep_type = u<width_keep>;
  using strb_type = u<width_strb>;
  using last_type = u<width_last>;
  using user_type = typename internal::axis_signal<has_user, WUser>::type;
  using id_type = typename internal::axis_signal<has_id, WId>::type;
  using dest_type = typename internal::axis_signal<has_dest, WDest>::type;

  // Declaration order is the vendor's, and every member offset depends on
  // it. Do not reorder.
  data_type data;
  keep_type keep;
  strb_type strb;
  user_type user;
  last_type last;
  id_type id;
  dest_type dest;

  /// A packet with every payload byte marked valid and TLAST clear, leaving
  /// the payload uninitialized -- the vendor's default state. Note tapa::u
  /// zero-initializes where the vendor's ap_uint is genuinely indeterminate,
  /// so a forgotten set_data reads as 0 here but garbage in Vitis
  /// simulation (and X in hardware): the friendly default hides that bug
  /// class, it does not excuse it.
  axis() {
    keep = -1;
    strb = -1;
    last = 0;
    internal::axis_set(user, u<width_user>(0));
    internal::axis_set(id, u<width_id>(0));
    internal::axis_set(dest, u<width_dest>(0));
  }

  axis(const data_type& data, const keep_type& keep, const strb_type& strb,
       const u<width_user>& user, const last_type& last, const u<width_id>& id,
       const u<width_dest>& dest)
      : data(data), keep(keep), strb(strb), last(last) {
    internal::axis_set(this->user, user);
    internal::axis_set(this->id, id);
    internal::axis_set(this->dest, dest);
  }

  /// Marks every payload byte valid.
  void keep_all() { keep = -1; }

  data_type get_data() const { return data; }
  void set_data(const data_type& value) { data = value; }
  keep_type get_keep() const { return keep; }
  void set_keep(const keep_type& value) { keep = value; }
  strb_type get_strb() const { return strb; }
  void set_strb(const strb_type& value) { strb = value; }
  last_type get_last() const { return last; }
  void set_last(const last_type& value) { last = value; }

  u<width_user> get_user() const { return internal::axis_get(user); }
  void set_user(const u<width_user>& value) { internal::axis_set(user, value); }
  u<width_id> get_id() const { return internal::axis_get(id); }
  void set_id(const u<width_id>& value) { internal::axis_set(id, value); }
  u<width_dest> get_dest() const { return internal::axis_get(dest); }
  void set_dest(const u<width_dest>& value) { internal::axis_set(dest, value); }
};

/// Signal-by-signal equality. A TAPA addition: the vendor packet has no
/// comparison operators.
template <typename T, int WUser, int WId, int WDest>
inline bool operator==(const axis<T, WUser, WId, WDest>& lhs,
                       const axis<T, WUser, WId, WDest>& rhs) {
  return lhs.data == rhs.data && lhs.keep == rhs.keep && lhs.strb == rhs.strb &&
         lhs.last == rhs.last && internal::axis_eq(lhs.user, rhs.user) &&
         internal::axis_eq(lhs.id, rhs.id) &&
         internal::axis_eq(lhs.dest, rhs.dest);
}

template <typename T, int WUser, int WId, int WDest>
inline bool operator!=(const axis<T, WUser, WId, WDest>& lhs,
                       const axis<T, WUser, WId, WDest>& rhs) {
  return !(lhs == rhs);
}

template <typename T, int WUser, int WId, int WDest>
inline std::ostream& operator<<(std::ostream& os,
                                const axis<T, WUser, WId, WDest>& obj) {
  os << "{data: " << obj.data << ", keep: " << obj.keep
     << ", strb: " << obj.strb << ", last: " << obj.last;
  internal::axis_print(os, "user", obj.user);
  internal::axis_print(os, "id", obj.id);
  internal::axis_print(os, "dest", obj.dest);
  return os << "}";
}

}  // namespace tapa
