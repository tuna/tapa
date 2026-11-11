# C++ API

This page documents the TAPA C++ library (`#include <tapa.h>`). Types and functions live in the `tapa` namespace unless noted otherwise.

---

## Task Invocation

### `tapa::invoke`

The host's entry point: runs the top-level task `f` on `args` and returns the
kernel time in nanoseconds (in software simulation, the wall-clock time of the
run). `f` names the task function itself, not a pointer to it.

```cpp
template <typename Func, typename... Args>
int64_t invoke(Func&& f, const std::string& bitstream, Args&&... args);
```

Each memory argument takes a
[directional wrapper](#directional-mmap-wrappers-host-side-only); a stream
argument of the top takes a `tapa::stream`, which XRT carries only for a
stream that reaches a frame port ([Frames from and to the
host](../concepts/streams.md#frames-from-and-to-the-host)) and VRT not at
all. The path selects the backend:

| `bitstream` | Backend |
|-------------|---------|
| empty | Software simulation |
| `.xo` or `.zip` | Fast cosimulation of the package's RTL |
| `.xclbin` | XRT: Vitis hardware emulation for an xclbin built for `hw_emu`, the board otherwise |
| `.vbin` | An Alveo V80 through VRT, or SLASH simulation for a vbin linked with `--sim` |
| a package directory from `tapa compile --aie-sim` | The AI Engine simulator, PL tasks in software |

Any other path is an error.

### `tapa::task`

The task hierarchy builder. An upper-level task constructs a `tapa::task` and chains `.invoke()` calls on it. The `tapa::task` destructor waits for all joined child instances to finish before returning.

```cpp
struct task {
  // Invoke func with the given arguments using the default join mode.
  template <typename Func, typename... Args>
  task& invoke(Func&& func, Args&&... args);

  // Invoke func with an explicit mode (tapa::join or tapa::detach).
  template <auto mode, typename Func, typename... Args>
  task& invoke(Func&& func, Args&&... args);

  // Invoke func N times with the given mode.
  template <auto mode, int N, typename Func, typename... Args>
  task& invoke(Func&& func, Args&&... args);
};
```

Each form also takes an instance name, a string literal, right after `func`:
`.invoke(Worker, "worker", args...)`. It names the instance in the generated
RTL and the floorplan, in place of the task name and index; with a repeat
count `N`, instance `i` is named `<name>_<i>`.

**Invoke modes:**

| Mode | Behavior |
|------|----------|
| `tapa::join` (default) | The task runs concurrently with siblings; the parent waits for it to finish before returning. |
| `tapa::detach` | Fire-and-forget; the parent does not wait for the task to finish. Use with care — the parent may return before the detached task completes. |

**Example:**

```cpp
void Top(tapa::istream<float>& in, tapa::ostream<float>& out, int n) {
  tapa::task()
      .invoke(LoadData, in, n)
      .invoke<tapa::detach>(MonitorTask, n)
      .invoke(StoreData, out, n);
}
```

### `tapa::seq`

A sequential index generator. When `tapa::seq{}` is passed as an argument to `.invoke()` with a repeat count `N`, each invocation receives a unique integer (0, 1, 2, …, N−1). Use this to distribute indexed work across task instances, such as assigning each instance its slice of a stream array.

```cpp
tapa::streams<float, 4> channels;
tapa::task().invoke<tapa::join, 4>(Worker, channels, tapa::seq{});
// Worker instance 0 gets channel[0], instance 1 gets channel[1], etc.
```

### `tapa::executable`

Wraps a kernel path for the host-only `tapa::task().invoke()` overload. An
empty path selects software simulation. A nonempty `.xo` or `.zip` starts
fast cosimulation; an `.xclbin` requests XRT execution. Parallel RTL
emulation uses XO or ZIP archives because the XRT path does not accept the
external stream ports used to connect those kernel instances.

```cpp
class executable {
 public:
  explicit executable(std::string path);
};
```

**Usage:**

```cpp
tapa::task().invoke(MyKernel, tapa::executable("my_kernel.xo"), arg1, arg2);
```

---

## Streams

Streams are the fundamental inter-task communication primitive. Each stream is a fixed-depth FIFO. Blocking operations stall until data or space is available; non-blocking operations return immediately.

For direct integer elements (`tapa::u<W>`, `tapa::i<W>`, `ap_uint<W>` and `ap_int<W>`), the task graph records the declared payload width `W`, independently of the C++ object's size and alignment. Internal FIFO data wires carry `W` data bits plus one EoT bit. Vitis top-level AXI4-Stream ports carry `8 * ceil(W / 8)` TDATA bits, with zero padding above the payload and EoT on TLAST. The unsigned QDMA envelope preserves signed payloads as their `W`-bit two's-complement pattern; all TKEEP bytes are valid.

### `tapa::stream<T, Depth>`

Owner type for a point-to-point FIFO. Declare inter-task channels inside an
upper-level task and pass directional views to children as `istream<T>&`
(read end) or `ostream<T>&` (write end). The default depth is 2.

```cpp
template <typename T, uint64_t Depth = 2>
class stream;
```

### `tapa::istream<T>`

Read-only view of a stream. Always passed by reference in task signatures: `tapa::istream<T>&`.

| Method | Blocking | Destructive | Description |
|--------|----------|-------------|-------------|
| `read()` | yes | yes | Blocks until an element is available, then returns it. |
| `read(bool& ok)` | no | yes | Non-blocking read; sets `ok` to true if an element was consumed. |
| `read(nullptr)` | no | yes | Non-blocking read that discards the success flag; returns `T()` when the stream is empty. Use after an `empty()` / `try_eot()` check has already established that data is there. |
| `read(U* dst, int n)` | yes | yes | Reads `n` values of `U` into `dst`. The sizes of `U` and `T` must divide one another, and the values line up with the elements byte for byte: a wider `U` takes `sizeof(U) / sizeof(T)` elements per value, and a narrower one takes `sizeof(T) / sizeof(U)` values from each element, so `n` must then be a multiple of that. A `U` that is itself an array, such as a row of a two-dimensional array, follows the same size rule, including elements that span multiple rows: pass the array, `read(rows, n)`, rather than a pointer to its first element, which HLS takes to cover only the first row. On an AI Engine frame port (`[[tapa::frame(N)]]`) it copies whole runs of a frame. In HLS, independent bulk transfers run concurrently. |
| `try_read(T& val)` | no | yes | Non-blocking read; returns true and writes to `val` if successful. |
| `operator>>(T& val)` | yes | yes | Blocking read into `val`; returns the stream so reads can be chained. Equivalent to `val = read()`. |
| `peek(bool& ok)` | no | no | Returns the next element without consuming it; sets `ok` to whether one was available. |
| `peek(nullptr)` | no | no | Peek that discards the success flag; returns `T()` when the stream is empty. |
| `peek(bool& ok, bool& is_eot)` | no | no | Peek that also reports whether the head element is the EoT marker. |
| `try_peek(T& val)` | no | no | Non-blocking peek; returns true if data was available. |
| `empty()` | no | no | Returns true if the stream contains no elements. |
| `try_eot(bool& is_eot)` | no | no | Returns true if a head element is available, and sets `is_eot` to whether it is the end-of-transaction marker. This is the primitive the EoT loop macros are built on. |
| `eot(bool& ok)` | no | no | Returns true if the head element is an end-of-transaction marker; sets `ok` to whether an element was available at all. Inverted argument order relative to `try_eot`. |
| `open()` | yes | yes | Blocks until a token is available, then consumes it. The token must be EoT; call after draining the transaction's data tokens. |
| `try_open()` | no | yes | Non-blocking variant of `open()`; consumes an available EoT token and returns true. |

```admonish warning
`read()`, `read(bool&)`, `try_read()`, `try_peek()`, `peek(bool&)`, and
`peek(nullptr)` are data-element operations: do not call them on an EoT
marker. Software simulation treats that misuse as fatal, but other targets may
not diagnose it. Check `try_eot()` first or use a `TAPA_WHILE_*_EOT` macro.
Use `peek(bool&, bool&)` when you need to inspect both the value and the EoT
flag.
```

### `tapa::ostream<T>`

Write-only view of a stream. Always passed by reference in task signatures: `tapa::ostream<T>&`.

| Method | Blocking | Destructive | Description |
|--------|----------|-------------|-------------|
| `write(const T& val)` | yes | yes | Blocks until space is available, then writes `val`. |
| `write(const U* src, int n)` | yes | yes | Writes `n` values of `U` from `src`, lined up with the elements as `read(dst, n)` lines them up. |
| `try_write(const T& val)` | no | yes | Non-blocking write; returns true if the element was written. |
| `operator<<(const T& val)` | yes | yes | Blocking write; returns the stream so writes can be chained. Equivalent to `write(val)`. |
| `full()` | no | no | Returns true if the stream is full. |
| `close()` | yes | yes | Writes an end-of-transaction marker; blocks until space is available. |
| `try_close()` | no | yes | Non-blocking variant of `close()`; returns true if the EoT was written. |

### `tapa::streams<T, N, Depth>`

Array of `N` streams of type `T`, each with depth `Depth`. Declared in an upper-level task and unpacked by index when passed to child tasks.

### `tapa::istreams<T, N>` / `tapa::ostreams<T, N>`

Array of `N` read-only or write-only stream views. Always passed by reference in task signatures.

### `tapa::iframes<T, N>` / `tapa::oframes<T, N>`

Frame ports of a leaf task: the stream moves frames of `N` elements, and the
task takes each one whole and reads (`iframes`) or writes (`oframes`) its
elements in any order. Always passed by reference; the argument is a
`tapa::stream<T, Depth>` the invoking task declares, or a stream port it
takes (an upper task, or the top's own stream argument). See
[Streams: frame ports](../concepts/streams.md#frame-ports).

| Member | Description |
|--------|-------------|
| `acquire()` | Blocks until a whole frame has arrived (`iframes`) or a frame is free to fill (`oframes`), and returns a view of it. A port holds one view at a time. |
| `view[i]` | Element `i`, `0 <= i < N`: a value of an `iframes` view; assignable only, `view[i] = x`, on an `oframes` view. A slot of an output frame the task does not write holds an unspecified value. |
| `view.release()` | Hands the frame on: the input frame is freed, the output frame enters the stream. The end of the view's scope releases it too. |

```admonish note
All stream types (`istream`, `ostream`, `istreams`, `ostreams`, `iframes`, `oframes`) must be passed **by reference** in task signatures. Passing by value is a compile error.
```

### `tapa::fence()`

Keeps every stream or memory access after the call behind every one before
it. Without it, synthesis orders accesses to different ports only by their
data: HLS may issue a read in the same cycle as an earlier write, or ahead of
it, and the AI Engine compiler may move a read above a write in an earlier
loop. A task whose read waits on its own earlier write, such as one in a ring,
then deadlocks in hardware, though software simulation completes. In HLS the
fence is a clock boundary (`ap_wait()`) that port accesses do not cross,
while arithmetic still may; in a pipelined loop it costs a stage, not
initiation interval. On the AI Engine it is a scheduler barrier that no
instruction crosses. In software simulation it is a no-op. A frame port's
view orders its own accesses: in HLS its release waits for every access to
the frame before it. See [AI Engine Tasks](../howto/aie.md).

---

## Memory (mmap)

### `tapa::mmap<T>`

A pointer-like handle for synchronous bulk memory access. Backed by a contiguous host allocation. In a task signature, `tapa::mmap<T>` is passed **by value**.

```cpp
template <typename T>
class mmap {
 public:
  explicit mmap(T* ptr);
  mmap(T* ptr, uint64_t size);
  template <typename Container>
  explicit mmap(Container& container);  // accepts std::vector etc.

  T* data() const;
  uint64_t size() const;

  template <uint64_t N>
  mmap<vec_t<T, N>> vectorized() const;  // reinterpret as wider element type

  template <typename U>
  mmap<U> reinterpret() const;  // reinterpret element type
};
```

This is the host's class. In HLS a task's `tapa::mmap<T>` parameter is a
plain `T*`, so task code uses only subscripts (`mem[i]`) and pointer
arithmetic on it.

### `tapa::async_mmap<T>`

Decoupled memory access type. The kernel issues read/write requests and
collects responses through five FIFO channels, giving it explicit control
over outstanding memory operations. Passed **by reference** in task
signatures: `tapa::async_mmap<T>&`.

See [async_mmap channels](#async_mmap-channels) below for channel details.

### `tapa::mmaps<T, N>`

Array of `N` `tapa::mmap<T>` regions. Passed by value as a single argument and unpacked by the framework one region per child invocation. Only an upper task takes one: `tapa synth` rejects a leaf task's `mmaps` parameter.

```cpp
template <typename T, uint64_t N>
class mmaps;
```

`slice<Offset, Count>()` returns a `mmaps<T, Count>` view of consecutive
regions. The range may end at `N`; each region retains its buffer address and
element count, so writes through the slice update the original buffer.

### Directional mmap wrappers (host-side only)

Used in the top-level `tapa::invoke()` call to express direction hints. The kernel task signature uses plain `tapa::mmap<T>` or `tapa::mmaps<T, N>`.

Wrappers may be temporary expressions or named mutable or const variables.
Each form passes the same buffer addresses, element counts, and direction hints.

| Wrapper | Direction |
|---------|-----------|
| `tapa::read_only_mmap<T>` | Host writes, kernel reads |
| `tapa::write_only_mmap<T>` | Kernel writes, host reads |
| `tapa::read_write_mmap<T>` | Both read and write |
| `tapa::placeholder_mmap<T>` | No host/device transfer; for an unused buffer |
| `tapa::placeholder_mmaps<T, N>` | Array variant of `placeholder_mmap` |
| `tapa::read_only_mmaps<T, N>` | Array variant of `read_only_mmap` |
| `tapa::write_only_mmaps<T, N>` | Array variant of `write_only_mmap` |
| `tapa::read_write_mmaps<T, N>` | Array variant of `read_write_mmap` |

### `tapa::aligned_allocator<T>`

STL-compatible allocator that returns page-aligned memory suitable for DMA transfers. Use this with `std::vector` when allocating host buffers that will be passed to a kernel.

```cpp
std::vector<float, tapa::aligned_allocator<float>> buf(n);
tapa::invoke(MyKernel, bitstream, tapa::read_only_mmap<float>(buf), n);
```

---

## async_mmap Channels

`tapa::async_mmap<T>` exposes five public member channels. The kernel writes addresses to the request channels and reads results from the response channels. All channel operations are non-blocking where prefixed with `try_`.

| Channel | Type | Direction | Description |
|---------|------|-----------|-------------|
| `read_addr` | request | kernel → memory | Write an element index (not a byte address) to request one element, or a `tapa::burst` `{offset, n}` to request `n` consecutive elements. |
| `read_data` | `istream<T>` | memory → kernel | Read the data returned by a previously issued read request. |
| `write_addr` | request | kernel → memory | Write an element index, or a `tapa::burst` `{offset, n}`, to request writes. |
| `write_data` | `ostream<T>` | kernel → memory | Write the data to be written at the requested addresses, in request order. |
| `write_resp` | `istream<uint8_t>` | memory → kernel | Drain write-completion acknowledgements. Each response value encodes `burst_length - 1` (i.e., a value of 0 means one write completed, 255 means 256 writes completed), however the requests were made: a long burst can take several responses. |

A request channel has `full()`, `try_write` and `write` for either kind of request. `tapa::burst` holds two `int64_t`, `offset` and `n`; a burst holds fewer than 2^32 elements.

```admonish warning
The kernel must drain `write_resp` to avoid deadlock. If the response channel fills up, the memory subsystem stops issuing further write completions and the kernel stalls.
```

**Typical async_mmap read pattern:**

```cpp
void Reader(tapa::async_mmap<float>& mem, tapa::ostream<float>& out, int n) {
  [[tapa::pipeline(1)]] for (int i_req = 0, i_resp = 0; i_resp < n;) {
    if (i_req < n && mem.read_addr.try_write(i_req)) {
      ++i_req;
    }
    float val;
    if (mem.read_data.try_read(val)) {
      out.write(val);
      ++i_resp;
    }
  }
}
```

---

## Utilities

### `tapa::vec_t<T, N>`

An N-element SIMD vector of type `T`. Stores elements as a packed bit array, which maps directly to wide AXI ports. Supports element access via `operator[]`, arithmetic operators element-wise, and common reductions (`sum`, `product`).

```cpp
template <typename T, int N>
struct vec_t {
  static constexpr int length = N;
  static constexpr int width = widthof<T>() * N;  // total bit width

  T& operator[](int pos);
  const T& operator[](int pos) const;
};
```

Related free functions: `truncated<begin, end>(vec)`, `cat(v1, v2)`, `make_vec<N>(val)`.

Lane shuffles, on every target including the AI Engine:
`interleave_zip<step>(a, b)` returns the lanes `{a[0:step], b[0:step],
a[step:2 * step], ...}` as a pair of vectors, `.first` holding the first `N`;
`interleave_unzip<step>(a, b)` inverts it, sending the runs of `step` lanes
of `{a, b}` alternately to `.first` and `.second`. `step` divides `N`. On a
native AI Engine shape each is one `aie::interleave_zip` /
`aie::interleave_unzip` shuffle; in HLS it is wiring.

Scalar compound assignments evaluate the scalar once before updating any
element, including when the scalar refers to the same vector. `shift(val)`
discards the first element and appends the value of `val` captured before the
shift.

### `tapa::u<W>` / `tapa::i<W>`

Portable arbitrary-width integers: `tapa::u<32>` is a 32-bit unsigned value
and `tapa::i<32>` a 32-bit signed one. Use the same types in software
simulation and synthesis; no vendor header is needed on the host.

```cpp
template <int W>
class u;  // unsigned
template <int W>
class i;  // signed
```

Semantics mirror the vendor types:

- Mixed-width arithmetic widens so the exact result fits (`tapa::u<8>(200) + tapa::u<8>(100)` is a `tapa::u<9>` holding 300); narrowing assignment truncates the bit pattern.
- Ordering follows C's usual arithmetic conversions with the declared width as the conversion rank: below 32 bits the comparison is signed however it is spelled (C promotes both operands to `int`), and at 32 bits and above the wider-or-equal operand's signedness wins, with an equal-width tie going unsigned, as in C. So `tapa::u<8>(255) > tapa::i<8>(-1)` is true, and `tapa::i<32>(-1) < tapa::u<32>(0)` is false.
- Equality compares bit patterns, not converted values: each operand is sign- or zero-extended into `max(width, 64)` bits and the patterns compare. So `tapa::i<64>(-1) == tapa::u<64>(~0)` is true, while `tapa::i<32>(-1) == tapa::u<32>(0xffffffff)` is false — unlike C, which would convert the signed operand and call it true.
- Mixed-sign division and remainder convert at one bit above the wider width, using the wider operand's signedness (signed on an equal-width tie). Division then truncates toward zero, and the remainder takes the converted dividend's sign.
- Shifting by a negative amount shifts the other way.

The full surface covers construction from builtins/floats, arithmetic and bitwise operators with builtin mixing, slicing (`x(hi, lo)`, `x.range<Hi, Lo>()`), bit select (`x[i]`, `x.set_bit(i, v)`), concatenation (`tapa::concat(a, b)` and the `(a, b)` form), reductions (`and_reduce`, `or_reduce`, `xor_reduce`, and their complements), conversions (`to_int64`, `to_uint64`, `to_double`, implicit `RetType`), and `reverse()`.

### `tapa::fixed<W, I, Q, O, N>` / `tapa::ufixed<W, I, Q, O, N>`

Portable fixed-point numbers, replacing `ap_fixed`/`ap_ufixed`. A value is a
`W`-bit two's-complement integer scaled by `2^-(W - I)`: `I` bits above the
binary point (including the sign bit for signed types), and `W - I` below.
Either count may exceed `W` or go negative. The same types work in software
simulation and synthesis.

```cpp
template <int W, int I, q_mode Q = q_mode::trn, o_mode O = o_mode::wrap,
          int N = 0>
class fixed;   // signed
template <int W, int I, q_mode Q = q_mode::trn, o_mode O = o_mode::wrap,
          int N = 0>
class ufixed;  // unsigned
```

`Q` says what happens to fractional bits the target cannot hold, `O` to a value that does not fit. Both mirror the vendor modes one for one, including their defaults:

| `tapa::q_mode` | vendor | meaning |
|---|---|---|
| `trn` (default) | `AP_TRN` | truncate toward minus infinity |
| `trn_zero` | `AP_TRN_ZERO` | truncate toward zero |
| `rnd` | `AP_RND` | round, ties toward plus infinity |
| `rnd_zero` | `AP_RND_ZERO` | round, ties toward zero |
| `rnd_min_inf` | `AP_RND_MIN_INF` | round, ties toward minus infinity |
| `rnd_inf` | `AP_RND_INF` | round, ties away from zero |
| `rnd_conv` | `AP_RND_CONV` | round, ties to even |

| `tapa::o_mode` | vendor | meaning |
|---|---|---|
| `wrap` (default) | `AP_WRAP` | keep the low bits; on overflow, apply the vendor's `N`-saturation-bit rule |
| `wrap_sm` | `AP_WRAP_SM` | sign-magnitude wrap; signed types only |
| `sat` | `AP_SAT` | clamp to the largest representable magnitude |
| `sat_zero` | `AP_SAT_ZERO` | replace with zero |
| `sat_sym` | `AP_SAT_SYM` | clamp to a range symmetric about zero |

Arithmetic never quantizes: `+`, `-` and `*` widen so the result is exact, and the result type carries the *default* modes rather than the operands'. The raw pattern is the public member `V`, and `x(hi, lo)` / `x[i]` slice it, as on the vendor type.

Notable behavior: `o_mode::wrap_sm` on an unsigned type is a compile error.
Comparisons follow TAPA's fixed-point conversion rules and can differ from a
comparison of the exact real values.

### Intentional subset

The portable types implement a commonly used subset. These vendor forms are
not implemented and fail at compile time: `tapa::fixed` shift operators and
its `&`/`|`/`^` against C integers, and the vendor's `to_string` hexfloat
formatting for fixed-point. `tapa::u/i` additionally offer
`x.to_string(base, sign)` and stream extraction (`in >> x`).

### `tapa::axis<T, WUser, WId, WDest>`

Vendor-agnostic AXI4-Stream packet, replacing `ap_axiu`/`ap_axis`. Parameterized by payload type, as the vendor's own `hls::axis` is: `ap_axiu<W, U, I, D>` becomes `tapa::axis<tapa::u<W>, U, I, D>` and `ap_axis<...>` becomes `tapa::axis<tapa::i<W>, ...>`.

```cpp
template <typename T, int WUser = 0, int WId = 0, int WDest = 0>
struct axis {
  T data;
  u<width_keep> keep;   // one bit per payload byte
  u<width_strb> strb;
  /* user */ last; /* id */ /* dest */
};
```

TDATA, TKEEP, TSTRB and TLAST are always present; TUSER, TID and TDEST are
present exactly when their width is non-zero. The layout is compatible with
`ap_axiu` for stream and mmap use. Accessing a disabled signal is inert.
`operator==` and `operator<<` are also provided.

The vendor's `EnableSignals` bit field, which turns TKEEP/TSTRB/TLAST off individually, has no portable form. `ap_axiu<W, U, I, D>` always has them.

### `tapa::wait()` / `tapa::wait(n)`

Yields one clock cycle on synthesis targets (lowered to the vendor `ap_wait()`), or `n` cycles for the overload (`ap_wait_n(n)`). Both are no-ops in software simulation, where tasks run as coroutines without a clock, and on the AI Engine. Use them in place of `ap_wait()` and `ap_wait_n()` to keep programs portable.

The simulation no-op is intentional: call `tapa::wait()` without a
`__SYNTHESIS__` guard so the same source works in software and synthesis.

### `tapa::widthof<T>()`

Returns the bit width of type `T`. For `tapa::u<W>`/`tapa::i<W>` (and `ap_int<W>`/`ap_uint<W>` on vendor targets), returns `W`. For plain C++ types, returns `sizeof(T) * CHAR_BIT`.

```cpp
template <typename T>
inline constexpr int widthof();

template <typename T>
inline constexpr int widthof(T object);  // deduce T from argument
```

### EoT macros

End-of-transaction macros simplify consuming a stream until a sentinel marker is received. See [Streams](../concepts/streams.md#eot-loop-helper-macros) for the usage pattern.

| Macro | Description |
|-------|-------------|
| `TAPA_WHILE_NOT_EOT(stream)` | Loop body executes once per data element; loop exits when the EoT marker is seen. |
| `TAPA_WHILE_NEITHER_EOT(s1, s2)` | Two-stream variant; exits when either stream reaches EoT. |
| `TAPA_WHILE_NONE_EOT(s1, s2, s3)` | Three-stream variant. |

### Synthesis pragmas (C++ attributes)

These C++ attributes are recognised by TAPA and lowered to vendor pragmas during synthesis. They have no effect in software simulation, and they are how a TAPA program expresses synthesis directives without naming a vendor: write these instead of `#pragma HLS ...`.

**On a statement.** Written on a loop, the attribute applies to *that* loop. `pipeline` and `unroll` must be written on the loop itself (or, for `pipeline`, on the function); the region attributes — `tripcount`, `flatten`, `latency`, `dependence`, `balance` — may also be written on an `if` (with braces, no `else`) or a bare block, applying to that region, which is what a bare pragma in the same position would have meant. A TAPA attribute on a subject it cannot lower to is a hard error, not a silent ignore. In code the preprocessor skips, which tapacc never parses, an attribute is removed with a remark instead: it takes no effect, whatever macros a vendor compiles the code with.

| Attribute | Lowers to | Description |
|-----------|-----------|-------------|
| `[[tapa::pipeline(II, style)]]` | `pipeline` | Pipeline the loop (or function) at initiation interval `II`. Both arguments optional; `style` is `"stp"` (stall), `"flp"` (flushable) or `"frp"` (free-running), the vendor default when omitted. `[[tapa::pipeline(false)]]` *disables* pipelining (`pipeline off`), the same way `flatten(false)` disables flattening. |
| `[[tapa::unroll(factor)]]` | `unroll` | Unroll by `factor`; fully unroll when omitted. |
| `[[tapa::tripcount(min, max)]]` | `loop_tripcount` | Declare the loop's trip-count range. Estimation only — it does not change generated hardware. On the AI Engine it is a promise instead: see [AI Engine Tasks](../howto/aie.md). |
| `[[tapa::flatten(enable)]]` | `loop_flatten` | `false` (or `0`) disables flattening of the loop nest; omitted leaves the vendor's automatic flattening on. |
| `[[tapa::latency(min, max)]]` | `latency` | Constrain the latency of the loop or region. `max = 0` requests a combinational region. |
| `[[tapa::dependence(var, class, type, direction, dependent, distance)]]` | `dependence` | Describe a loop-carried dependence on `var`, in the vendor's argument order. `class` is `"array"`/`"pointer"`, `type` `"inter"`/`"intra"`, `direction` `"RAW"`/`"WAR"`/`"WAW"` — all validated at parse time; `dependent` non-zero asserts a real dependence at `distance` (a string for macro passthrough or an integer constant), the default asserts independence. All but `var` are optional; use `""` for a skipped position. |
| `[[tapa::balance]]` | `expression_balance` | Re-associate the expression tree in the region. |

**On a variable or parameter declaration.** These take the declaration they annotate; on a multi-declarator statement each variable gets its own pragma.

| Attribute | Lowers to | Description |
|-----------|-----------|-------------|
| `[[tapa::partition(type, factor, dim)]]` | `array_partition` | Partition an array: `type` is `"cyclic"`, `"block"` or `"complete"`. `factor` and `dim` are positional and optional — pass `-1` to omit one you are skipping, e.g. `[[tapa::partition("complete", -1, 2)]]` partitions dimension 2 with no factor. A factor with `"complete"` is rejected at parse time: it is meaningless, and the vendor would silently partition the wrong dimension. |
| `[[tapa::storage(type, impl, latency)]]` | `bind_storage` | Choose the memory the array binds to, e.g. `[[tapa::storage("RAM_2P", "URAM")]]`. |
| `[[tapa::aggregate]]` | `aggregate` | Pack a struct into a single wide word. |
| `[[tapa::bind_op(op, impl, latency)]]` | `bind_op` | Bind an operation to a specific implementation, e.g. `("mul", "dsp")`. |
| `[[tapa::array_map(instance, offset, orient)]]` | `array_map` | Map an array into a larger physical instance. `offset` is positional and optional — `-1` omits it. |
| `[[tapa::frame(N)]]` | — | On a stream parameter of a leaf task: the port moves whole frames of `N` elements, and `tapa analyze` rejects a stream that holds fewer, on every target. On the AI Engine the frames arrive in tile memory; final partial output frames are zero-padded. See [AI Engine Tasks](../howto/aie.md). |
| `[[tapa::ring(S)]]` | — | On a stream parameter of the top that reaches a frame port or a `[[tapa::frame(N)]]` stream port, `S >= 1`: on Vitis the ring in device memory that carries it holds `S` frames instead of 4; on an AI Engine top's, the ring it gets on a VCK5000 (unused under GMIO); the other flows have no ring. Anywhere else an error. See [Streams: frames from and to the host](../concepts/streams.md#frames-from-and-to-the-host). |

**On a function.**

| Attribute | Description |
|-----------|-------------|
| `[[tapa::target("hls")]]` | Build the task with HLS in the design's flow. This is the default; write it to keep a task off the AI Engine, including one an AI Engine upper task invokes. |
| `[[tapa::target("ignore")]]` | Mark a task for custom RTL replacement. TAPA generates a port-signature template but does not synthesize the task body. |
| `[[tapa::target("aie")]]` | Run the task on the Versal AI Engine array. On an upper task it is the default for the tasks below it that have no target of their own. See [AI Engine Tasks](../howto/aie.md). |

```admonish note
The `-1` sentinel in `partition` and `array_map` exists because the arguments are positional and zero is a meaningful value (`dim = 0` means *all* dimensions). Passing a dim where a factor is expected is silently accepted by the vendor and partitions the wrong dimension, so spell the sentinel rather than dropping the argument.
```

```admonish note
`tapa::reg<T>(x)` returns `x` through a pipeline register, and
`tapa::reg<T, Depth>(x)` through `Depth` of them; software simulation and the
AI Engine return `x` unchanged. It replaces the hand-rolled
`HLS_REG` helper that vendor designs carry (a no-inline function with
`pipeline` and `interface ... register` pragmas). Note it is a real
register: a design whose `HLS_REG` registered only the return *interface*
will use more flip-flops after the switch.
```

```admonish note
One vendor construct has no attribute form: `#pragma HLS dataflow` *inside*
a leaf task, for concurrency between plain function calls. TAPA expresses
concurrency through the task graph, so the portable equivalent is to invoke
those functions as tasks (`tapa::task().invoke(...)`). Where a design
deliberately keeps a leaf task that drives its own internal stream, the
pragma stays and `tapa analyze` reports it as a remark.
```

```admonish tip
Function-level inlining is driven by the C++ `inline` keyword, not a TAPA
attribute. Mark a helper `inline` to inline it; omit the keyword to prevent
inlining. Do not hand-write HLS inline pragmas.
```

### `tapa::hls` sub-namespace

`tapa::hls::stream<T, N>` has hardware FIFO depth `N` (default 2) but
effectively infinite depth in software simulation. It is useful when
migrating leaf-local Vitis dataflow code because producers do not block on a
full software queue. `#include <tapa.h>` includes the alias.

This alias does not change TAPA task-port rules: a task boundary still uses
`tapa::istream<T>&` or `tapa::ostream<T>&`, not `stream` or `tapa::hls::stream`.
It is not necessary to replace leaf-local uses before synthesis, but switching
to `tapa::stream<T, N>` during debugging enforces the hardware depth in
software simulation and can expose backpressure bugs.
