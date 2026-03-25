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
| `try_read(T& val)` | no | yes | Non-blocking read; returns true and writes to `val` if successful. |
| `peek(bool& ok)` | no | no | Returns the next element without consuming it; sets `ok`. |
| `try_peek(T& val)` | no | no | Non-blocking peek; returns true if data was available. |
| `empty()` | no | no | Returns true if the stream contains no elements. |
| `eot(bool& ok)` | no | no | Returns true if the head element is an end-of-transaction marker. |
| `open()` | yes | yes | Blocks until an EoT marker arrives, then consumes it. Used to receive stream closure. |
| `try_open()` | no | yes | Non-blocking variant of `open()`; returns true if EoT was consumed. |

### `tapa::ostream<T>`

Write-only view of a stream. Always passed by reference in task signatures: `tapa::ostream<T>&`.

| Method | Blocking | Destructive | Description |
|--------|----------|-------------|-------------|
| `write(const T& val)` | yes | yes | Blocks until space is available, then writes `val`. |
| `try_write(const T& val)` | no | yes | Non-blocking write; returns true if the element was written. |
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
#pragma HLS pipeline II=1
  for (int i_req = 0, i_resp = 0; i_resp < n;) {
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

### `tapa::widthof<T>()`

Returns the bit width of type `T`. For `ap_int<W>` and `ap_uint<W>`, returns `W`. For plain C++ types, returns `sizeof(T) * CHAR_BIT`.

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

These C++ attributes are recognised by TAPA and lowered to Vitis HLS pragmas during synthesis. They have no effect in software simulation.

| Attribute | Description |
|-----------|-------------|
| `[[tapa::pipeline(II)]]` | Pipeline the enclosing loop or function with initiation interval `II`. |
| `[[tapa::unroll(factor)]]` | Unroll the enclosing loop by `factor`. |
| `[[tapa::target("ignore")]]` | Mark a task for custom RTL replacement. TAPA generates a port-signature template but does not synthesize the task body. |

```admonish note
`[[tapa::target("ignore")]]` was formerly written as `[[tapa::target("non_synthesizable", "xilinx")]]`. The `"ignore"` form is the current spelling.
```

### `tapa::hls` sub-namespace

`tapa::hls::stream<T>` is a stream type that behaves like `hls::stream<T>` in software simulation: it has effectively infinite depth, so producers never block in simulation. Use it when incrementally migrating a Vitis HLS design and you want software simulation to pass without tuning stream depths. `#include <tapa.h>` includes this automatically.

```admonish warning
`tapa::hls::stream` is **not synthesizable** for use as a direct replacement for `hls::stream`. Before targeting hardware, replace all `tapa::hls::stream` uses with `tapa::istream<T>&` / `tapa::ostream<T>&` and tune stream depths appropriately.
```
