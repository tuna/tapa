# C++ API

This page documents the TAPA C++ library (`#include <tapa.h>`). Types and functions live in the `tapa` namespace unless noted otherwise.

---

## Task Invocation

### `tapa::task`

The task hierarchy builder. An upper-level task constructs a `tapa::task` and chains `.invoke()` calls on it. The `tapa::task` destructor waits for all joined child instances to finish before returning.

```cpp
struct task {
  // Invoke func with the given arguments using the default join mode.
  template <typename Func, typename... Args>
  task& invoke(Func&& func, Args&&... args);

  // Invoke func with an explicit mode (tapa::join or tapa::detach).
  template <internal::InvokeMode mode, typename Func, typename... Args>
  task& invoke(Func&& func, Args&&... args);

  // Invoke func N times with the given mode.
  template <internal::InvokeMode mode, int N, typename Func, typename... Args>
  task& invoke(Func&& func, Args&&... args);
};
```

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

Wraps a path to an XO or bitstream file for use in `.invoke()`. When an `executable` is passed as the second argument to `.invoke()`, the task runs on hardware (via FRT) instead of in software simulation.

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

### `tapa::stream<T, Depth>`

Bidirectional FIFO that owns the underlying storage. Declared inside an upper-level task and passed to child tasks as `istream<T>&` (read end) or `ostream<T>&` (write end). The default depth is 2.

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
| `try_read(T& val)` | no | yes | Non-blocking read; returns true and writes to `val` if successful. |
| `operator>>(T& val)` | yes | yes | Blocking read into `val`; returns the stream so reads can be chained. Equivalent to `val = read()`. |
| `peek(bool& ok)` | no | no | Returns the next element without consuming it; sets `ok` to whether one was available. |
| `peek(nullptr)` | no | no | Peek that discards the success flag; returns `T()` when the stream is empty. |
| `peek(bool& ok, bool& is_eot)` | no | no | Peek that also reports whether the head element is the EoT marker. |
| `try_peek(T& val)` | no | no | Non-blocking peek; returns true if data was available. |
| `empty()` | no | no | Returns true if the stream contains no elements. |
| `try_eot(bool& is_eot)` | no | no | Returns true if a head element is available, and sets `is_eot` to whether it is the end-of-transaction marker. This is the primitive the EoT loop macros are built on. |
| `eot(bool& ok)` | no | no | Returns true if the head element is an end-of-transaction marker; sets `ok` to whether an element was available at all. Inverted argument order relative to `try_eot`. |
| `open()` | yes | yes | Blocks until an EoT marker arrives, then consumes it. Used to receive stream closure. |
| `try_open()` | no | yes | Non-blocking variant of `open()`; returns true if EoT was consumed. |

```admonish warning
`read()`, `read(bool&)`, `try_read()`, and `peek()` all abort on an EoT
marker — they are for data elements only. Check `try_eot()` first, or use one
of the `TAPA_WHILE_*_EOT` macros, when the producer closes the stream.
```

### `tapa::ostream<T>`

Write-only view of a stream. Always passed by reference in task signatures: `tapa::ostream<T>&`.

| Method | Blocking | Destructive | Description |
|--------|----------|-------------|-------------|
| `write(const T& val)` | yes | yes | Blocks until space is available, then writes `val`. |
| `try_write(const T& val)` | no | yes | Non-blocking write; returns true if the element was written. |
| `operator<<(const T& val)` | yes | yes | Blocking write; returns the stream so writes can be chained. Equivalent to `write(val)`. |
| `full()` | no | no | Returns true if the stream is full. |
| `close()` | yes | yes | Writes an end-of-transaction marker; blocks until space is available. |
| `try_close()` | no | yes | Non-blocking variant of `close()`; returns true if the EoT was written. |

### `tapa::streams<T, N, Depth>`

Array of `N` streams of type `T`, each with depth `Depth`. Declared in an upper-level task and unpacked by index when passed to child tasks.

### `tapa::istreams<T, N>` / `tapa::ostreams<T, N>`

Array of `N` read-only or write-only stream views. Always passed by reference in task signatures.

```admonish note
All stream types (`istream`, `ostream`, `istreams`, `ostreams`) must be passed **by reference** in task signatures. Passing by value is a compile error.
```

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

### `tapa::async_mmap<T>`

Decoupled memory access type. Instead of blocking on each memory operation, the kernel issues read/write requests and collects responses through five FIFO channels. This allows the kernel to pipeline memory operations. Passed **by reference** in task signatures: `tapa::async_mmap<T>&`.

See [async_mmap channels](#async_mmap-channels) below for channel details.

### `tapa::mmaps<T, N>`

Array of `N` `tapa::mmap<T>` regions. Passed by value as a single argument and unpacked by the framework one region per child invocation.

```cpp
template <typename T, uint64_t N>
class mmaps;
```

### Directional mmap wrappers (host-side only)

Used in the top-level `tapa::invoke()` call to express direction hints. The kernel task signature uses plain `tapa::mmap<T>` or `tapa::mmaps<T, N>`.

| Wrapper | Direction |
|---------|-----------|
| `tapa::read_only_mmap<T>` | Host writes, kernel reads |
| `tapa::write_only_mmap<T>` | Kernel writes, host reads |
| `tapa::read_write_mmap<T>` | Both read and write |
| `tapa::placeholder_mmap<T>` | No direction hint |
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
| `read_addr` | `ostream<int64_t>` | kernel → memory | Write an element index to request a read. The framework converts the index to a byte offset internally. |
| `read_data` | `istream<T>` | memory → kernel | Read the data returned by a previously issued read request. |
| `write_addr` | `ostream<int64_t>` | kernel → memory | Write an element index to request a write. |
| `write_data` | `ostream<T>` | kernel → memory | Write the data to be written at the requested address. |
| `write_resp` | `istream<uint8_t>` | memory → kernel | Drain write-completion acknowledgements. Each response value encodes `burst_length - 1` (i.e., a value of 0 means one write completed, 255 means 256 writes completed). |

```admonish warning
The kernel must drain `write_resp` to avoid deadlock. If the response channel fills up, the memory subsystem stops issuing further write completions and the kernel stalls.
```

**Typical async_mmap read pattern:**

```cpp
void Reader(tapa::async_mmap<float>& mem, tapa::ostream<float>& out, int n) {
  [[tapa::pipeline(1)]] for (int i_req = 0, i_resp = 0; i_resp < n;) {
    if (i_req < n && !mem.read_addr.full()) {
      mem.read_addr.write(i_req);
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

### `tapa::u<W>` / `tapa::i<W>`

Vendor-agnostic arbitrary-width integers: `tapa::u<32>` is a 32-bit unsigned value, `tapa::i<32>` a 32-bit signed one. In software simulation they are self-implemented (no vendor headers needed); on the Xilinx HLS target they alias `ap_uint<W>`/`ap_int<W>`.

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
- Division truncates toward zero; the remainder takes the dividend's sign.
- Shifting by a negative amount shifts the other way.

The full surface covers construction from builtins/floats, arithmetic and bitwise operators with builtin mixing, slicing (`x(hi, lo)`, `x.range<Hi, Lo>()`), bit select (`x[i]`, `x.set_bit(i, v)`), concatenation (`tapa::concat(a, b)` and the `(a, b)` form), reductions (`and_reduce`, `or_reduce`, `xor_reduce`, and their complements), conversions (`to_int64`, `to_uint64`, `to_double`, implicit `RetType`), and `reverse()`.

### `tapa::fixed<W, I, Q, O, N>` / `tapa::ufixed<W, I, Q, O, N>`

Vendor-agnostic fixed-point numbers, replacing `ap_fixed`/`ap_ufixed`. A value is a `W`-bit two's-complement integer scaled by `2^-(W - I)`: `I` bits above the binary point (the sign bit among them when signed), `W - I` below. Either count may exceed `W` or go negative. In software simulation they are self-implemented; on the Xilinx HLS target they alias the vendor types, because fixed-point arithmetic is something the HLS compiler implements natively.

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
| `wrap` (default) | `AP_WRAP` | keep the low bits; `N` saturation bits are forced |
| `wrap_sm` | `AP_WRAP_SM` | sign-magnitude wrap; signed types only |
| `sat` | `AP_SAT` | clamp to the largest representable magnitude |
| `sat_zero` | `AP_SAT_ZERO` | replace with zero |
| `sat_sym` | `AP_SAT_SYM` | clamp to a range symmetric about zero |

Arithmetic never quantizes: `+`, `-` and `*` widen so the result is exact, and the result type carries the *default* modes rather than the operands'. The raw pattern is the public member `V`, and `x(hi, lo)` / `x[i]` slice it, as on the vendor type.

Two deliberate differences. `o_mode::wrap_sm` on an unsigned type is a compile error; the vendor rejects the same combination at run time. And comparison is transcribed from the vendor, which widens whichever side has the coarser fractional width and then compares the *raw* integers — so it inherits the integer conversion rules above rather than comparing exact values.

### Intentional subset

The portable types cover the surface the regression designs and typical kernels use. The vendor forms below are *not* implemented, so code that needs them fails to compile (loudly, never silently): `tapa::fixed` shift operators and its `&`/`|`/`^` against C integers, and the vendor's `to_string` hexfloat formatting for fixed-point. `tapa::u/i` additionally offer `x.to_string(base, sign)` and stream extraction (`in >> x`) matching the vendor's member forms.

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

TDATA, TKEEP, TSTRB and TLAST are always present; TUSER, TID and TDEST are present exactly when their width is non-zero. Object size, alignment and every member offset match `ap_axiu`, which is what mmap element stride and stream element size depend on. A disabled signal still occupies its slot, so enabling one moves nothing.

Unlike `tapa::u`/`tapa::i` this is one definition for every target rather than a vendor alias: its members are `tapa::u<W>`, which *is* `ap_uint<W>` when synthesizing. Reading or writing a signal the packet does not carry is inert, where the vendor throws; `operator==` and `operator<<` are additions the vendor packet has not.

The vendor's `EnableSignals` bit field, which turns TKEEP/TSTRB/TLAST off individually, has no portable form. `ap_axiu<W, U, I, D>` always has them.

### `tapa::wait()` / `tapa::wait(n)`

Yields one clock cycle on synthesis targets (lowered to the vendor `ap_wait()`), or `n` cycles for the overload (`ap_wait_n(n)`). Both are no-ops in software simulation, where tasks run as coroutines without a clock. Use them in place of `ap_wait()` and `ap_wait_n()` to keep programs portable.

The simulation no-op is contractual: call `tapa::wait()` bare, never inside `#ifdef __SYNTHESIS__` — the guard exists only for the vendor intrinsics, which the host headers deliberately do not declare.

### `tapa::widthof<T>()`

Returns the bit width of type `T`. For `tapa::u<W>`/`tapa::i<W>` (and `ap_int<W>`/`ap_uint<W>` on vendor targets), returns `W`. For plain C++ types, returns `sizeof(T) * CHAR_BIT`.

```cpp
template <typename T>
inline constexpr int widthof();

template <typename T>
inline constexpr int widthof(T object);  // deduce T from argument
```

### EoT macros

End-of-transaction macros simplify consuming a stream until a sentinel marker is received.

| Macro | Description |
|-------|-------------|
| `TAPA_WHILE_NOT_EOT(stream)` | Loop body executes once per data element; loop exits when the EoT marker is seen. |
| `TAPA_WHILE_NEITHER_EOT(s1, s2)` | Two-stream variant; exits when either stream reaches EoT. |
| `TAPA_WHILE_NONE_EOT(s1, s2, s3)` | Three-stream variant. |

```cpp
// Example: consume all elements from 'in' and forward to 'out'
TAPA_WHILE_NOT_EOT(in) {
  out.write(in.read());
}
in.open();   // consume the EoT marker
out.close(); // send EoT marker downstream
```

### Synthesis pragmas (C++ attributes)

These C++ attributes are recognised by TAPA and lowered to vendor pragmas during synthesis. They have no effect in software simulation, and they are how a TAPA program expresses synthesis directives without naming a vendor: write these instead of `#pragma HLS ...`.

**On a statement.** Written on a loop, the attribute applies to *that* loop. `pipeline` and `unroll` must be written on the loop itself (or, for `pipeline`, on the function); the region attributes — `tripcount`, `flatten`, `latency`, `dependence`, `balance` — may also be written on an `if` (with braces, no `else`) or a bare block, applying to that region, which is what a bare pragma in the same position would have meant. A TAPA attribute on a subject it cannot lower to is a hard error, not a silent ignore.

| Attribute | Lowers to | Description |
|-----------|-----------|-------------|
| `[[tapa::pipeline(II, style)]]` | `pipeline` | Pipeline the loop (or function) at initiation interval `II`. Both arguments optional; `style` is `"stp"` (stall), `"flp"` (flushable) or `"frp"` (free-running), the vendor default when omitted. `[[tapa::pipeline(false)]]` *disables* pipelining (`pipeline off`), the same way `flatten(false)` disables flattening. |
| `[[tapa::unroll(factor)]]` | `unroll` | Unroll by `factor`; fully unroll when omitted. |
| `[[tapa::tripcount(min, max)]]` | `loop_tripcount` | Declare the loop's trip-count range. Estimation only — it does not change generated hardware. |
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

**On a function.**

| Attribute | Description |
|-----------|-------------|
| `[[tapa::target("ignore")]]` | Mark a task for custom RTL replacement. TAPA generates a port-signature template but does not synthesize the task body. |

```admonish note
The `-1` sentinel in `partition` and `array_map` exists because the arguments are positional and zero is a meaningful value (`dim = 0` means *all* dimensions). Passing a dim where a factor is expected is silently accepted by the vendor and partitions the wrong dimension, so spell the sentinel rather than dropping the argument.
```

```admonish note
`tapa::reg<T>(x)` returns `x` through a pipeline register, and
`tapa::reg<T, Depth>(x)` through `Depth` of them. It replaces the hand-rolled
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
Function-level inlining is driven by the `inline` keyword, not an attribute: a helper marked `inline` gets `#pragma HLS inline` plus `__attribute__((always_inline))`, and one without it gets `inline off` plus `noinline`. Do not hand-write those pragmas.
```

```admonish note
`[[tapa::target("ignore")]]` was formerly written as `[[tapa::target("non_synthesizable", "xilinx")]]`. The `"ignore"` form is the current spelling.
```

### `tapa::hls` sub-namespace

`tapa::hls::stream<T>` is a stream type that behaves like `hls::stream<T>` in software simulation: it has effectively infinite depth, so producers never block in simulation. Use it when incrementally migrating a Vitis HLS design and you want software simulation to pass without tuning stream depths. `#include <tapa.h>` includes this automatically.

```admonish note
`tapa::hls::stream` synthesizes to the same RTL FIFO as `tapa::stream<T, N>` with the declared depth `N`. The infinite depth only applies to software simulation. The practical reason to replace it before hardware build is that software simulation with `tapa::hls::stream` will not expose backpressure bugs — switching to `tapa::istream<T>&` / `tapa::ostream<T>&` with a tuned depth catches those bugs at simulation time rather than on hardware.
```
