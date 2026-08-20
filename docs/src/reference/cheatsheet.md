# C++ Quick Reference

Common patterns for writing TAPA kernels. For full API details see [C++ API](api.md).

---

## Task structure

```cpp
// Upper-level task: declare streams, invoke leaf tasks. No computation.
void Top(tapa::mmap<const float> in, tapa::mmap<float> out, uint64_t n) {
  tapa::stream<float, 16> q("q");
  tapa::task()
      .invoke(Load, in, n, q)
      .invoke(Store, q, out, n);
}

// Leaf task: contains all computation.
void Load(tapa::mmap<const float> mem, uint64_t n, tapa::ostream<float>& q) {
  for (uint64_t i = 0; i < n; ++i) q.write(mem[i]);
}

void Store(tapa::istream<float>& q, tapa::mmap<float> mem, uint64_t n) {
  for (uint64_t i = 0; i < n; ++i) mem[i] = q.read();
}
```

---

## Host code

```cpp
#include <gflags/gflags.h>
#include <tapa.h>

DEFINE_string(bitstream, "", "XO or xclbin path. Empty = software simulation.");

int main(int argc, char* argv[]) {
  gflags::ParseCommandLineFlags(&argc, &argv, true);

  std::vector<float, tapa::aligned_allocator<float>> a(n), b(n);

  tapa::invoke(Top, FLAGS_bitstream,
               tapa::read_only_mmap<const float>(a),
               tapa::write_only_mmap<float>(b),
               (uint64_t)n);
}
```

| `FLAGS_bitstream` value | Backend |
|-------------------------|---------|
| *(empty)* | Software simulation |
| `kernel.xo` | Fast cosimulation |
| `kernel.hw.xclbin` | On-board execution |

---

## Stream types

| Type | Use in signature | Direction |
|------|-----------------|-----------|
| `tapa::stream<T, Depth>` | local variable in upper task | owner |
| `tapa::istream<T>&` | leaf task parameter | read only |
| `tapa::ostream<T>&` | leaf task parameter | write only |
| `tapa::streams<T, N>` | local variable | array owner |
| `tapa::istreams<T, N>&` | leaf task parameter | array read |
| `tapa::ostreams<T, N>&` | leaf task parameter | array write |

```cpp
// Read
T val = in.read();             // blocking
bool ok = in.try_read(val);    // non-blocking, returns true on success

// Write
out.write(val);                // blocking
bool ok = out.try_write(val);  // non-blocking

// State checks
bool e = in.empty();
bool f = out.full();

// End-of-transaction
out.close();                   // send EoT marker
in.open();                     // consume EoT marker
TAPA_WHILE_NOT_EOT(in) { ... } // loop until EoT
```

Stream depth and FPGA resource:

| Depth | Resource |
|-------|----------|
| < 128 | SRL shift-register (no BRAM) |
| ≥ 128 | BRAM |
| ≥ 4096 and element width ≥ 36 b | URAM |

---

## Memory types

| Type | Signature | Access style |
|------|-----------|-------------|
| `tapa::mmap<T>` | by value | synchronous, pointer-like |
| `tapa::async_mmap<T>` | by reference `&` | decoupled AXI channels |

```cpp
// mmap — simple loop
for (int i = 0; i < n; ++i) out[i] = in[i];

// async_mmap — overlapping reads (two-counter loop)
[[tapa::pipeline(1)]] for (int64_t i_req = 0, i_resp = 0; i_resp < n;) {
  if (i_req < n) mem.read_addr.try_write(i_req++);
  T val;
  if (mem.read_data.try_read(val)) result[i_resp++] = val;
}

// async_mmap — writes with response drain
[[tapa::pipeline(1)]] for (int64_t i_req = 0, i_resp = 0; i_resp < n;) {
  if (i_req < n && !src.empty() &&
      !mem.write_addr.full() && !mem.write_data.full()) {
    mem.write_addr.try_write(i_req);
    mem.write_data.try_write(src.read(nullptr));
    ++i_req;
  }
  uint8_t ack;
  if (mem.write_resp.try_read(ack)) i_resp += unsigned(ack) + 1;
}
```

---

## Parallel task instances

```cpp
// Invoke N instances; each gets a unique index via tapa::seq
tapa::streams<float, 4> ch("ch");
tapa::task().invoke<tapa::join, 4>(Worker, ch, tapa::seq{});

void Worker(tapa::istream<float>& in, int idx) { /* ... */ }
```

---

## Synthesis attributes

Write these instead of `#pragma HLS ...`; on a loop the attribute applies to
that loop. `tapa analyze` warns when a vendor pragma is used and names the
portable form.

```cpp
// On the loop itself (or the function, for pipeline); the region
// attributes (tripcount, flatten, latency, dependence, balance) may also
// sit on a braced `if` or block to scope to that region.
[[tapa::pipeline(1)]]                     // initiation interval 1
[[tapa::pipeline(false)]]                 // do NOT pipeline this loop
[[tapa::unroll(4)]]                       // partially unroll; omit to unroll fully
[[tapa::tripcount(1, 800)]]               // trip-count estimate only
[[tapa::flatten(false)]]                  // do not flatten this nest
[[tapa::latency(1, 2)]]                   // constrain region latency
[[tapa::dependence("buf", "", "inter", "", 1, 8)]]  // real dependence, distance 8
[[tapa::balance]]                         // re-associate the expression tree

// On a variable or parameter declaration
[[tapa::partition("cyclic", 16)]]         // factor 16, dimension left to the vendor
[[tapa::partition("complete", -1, 2)]]    // dimension 2, no factor (-1 omits it)
[[tapa::storage("RAM_2P", "URAM")]]       // bind the array to URAM
[[tapa::aggregate]]                       // pack a struct into one wide word
[[tapa::bind_op("mul", "dsp")]]           // bind an operation to an implementation
[[tapa::array_map("inst", -1, "vertical")]]

// On a function
[[tapa::target("ignore")]]                // mark task for custom RTL replacement
```

Inlining follows the `inline` keyword, not an attribute: `inline` emits the
inline pragma plus `always_inline`, and its absence emits `inline off` plus
`noinline`.

---

## End-of-transaction macros

```cpp
TAPA_WHILE_NOT_EOT(in)          { out.write(in.read(nullptr)); }
TAPA_WHILE_NEITHER_EOT(in1,in2) { /* both have data */ }
TAPA_WHILE_NONE_EOT(a, b, c)    { /* all three have data */ }
```

---

## Build and run

```bash
# Software simulation
tapa g++ -- kernel.cpp host.cpp -o app
./app

# RTL synthesis
tapa compile --top Top --part-num xcu55c-fsvh2892-2L-e \
  --clock-period 3.33 -f kernel.cpp -o kernel.xo

# Fast cosimulation
./app --bitstream=kernel.xo

# Bitstream link (v++)
v++ -o app.hw.xclbin --link --target hw --kernel Top \
  --platform xilinx_u55c_gen3x16_xdma_3_202210_1 kernel.xo

# On-board run
./app --bitstream=app.hw.xclbin
```
