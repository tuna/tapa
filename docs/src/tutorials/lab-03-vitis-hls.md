# Lab 3: Migrating from Vitis HLS

**Goal:** Port an existing Vitis HLS kernel to TAPA by replacing HLS-specific constructs with their TAPA equivalents.

**Prerequisites:** [Lab 1: Vector Addition](lab-01-vadd.md) and familiarity with the TAPA task model.

After this lab you will understand:
- The mechanical substitutions that cover most Vitis HLS kernels
- Why the dataflow-in-a-loop pattern must be restructured in TAPA
- How `tapa::hls::stream` supports incremental migration of large codebases

---

## Quick reference: Vitis HLS → TAPA

| Vitis HLS | TAPA | Notes |
|-----------|------|-------|
| `#include <hls_stream.h>` | `#include <tapa.h>` | TAPA includes its own stream types |
| `T* port` + `#pragma HLS INTERFACE m_axi` | `tapa::mmap<T> port` (by value) | Remove all `m_axi` pragmas |
| `hls::stream<T>&` | `tapa::istream<T>&` or `tapa::ostream<T>&` | Direction is explicit in TAPA |
| `#pragma HLS dataflow` + direct calls | `tapa::task().invoke(...)` | Tasks run concurrently |
| Top function contains computation | Move computation into child tasks | TAPA upper-level tasks are orchestration-only |
| `hls::stream<T>` local variable | `tapa::stream<T>` local variable | Same syntax; depth is enforced during software simulation (default depth: 2) |

---

## Example 1: Basic VecAdd migration

The full before and after files are at [example_1_before.cpp](code/vitis-hls/example_1_before.cpp) and [example_1_after.cpp](code/vitis-hls/example_1_after.cpp).

### Step 1: Replace the include

```diff
-#include <hls_stream.h>
-#include <hls_vector.h>
+#include <hls_vector.h>
+#include <tapa.h>
```

TAPA provides its own stream types, so `hls_stream.h` is no longer needed. Other HLS headers such as `ap_int.h` and `hls_vector.h` are still supported and can be included as usual.

### Step 2: Replace pointer arguments with `tapa::mmap<T>`

Vitis HLS uses raw pointers annotated with `#pragma HLS INTERFACE m_axi` to indicate off-chip memory. TAPA replaces this with `tapa::mmap<T>` passed by value, and no pragma is needed:

```diff
-void load_input(hls::vector<uint32_t, NUM_WORDS>* in,
+void load_input(tapa::mmap<hls::vector<uint32_t, NUM_WORDS>> in,
```

```diff
-  hls::vector<uint32_t, NUM_WORDS>* in1,
-  hls::vector<uint32_t, NUM_WORDS>* in2,
-  hls::vector<uint32_t, NUM_WORDS>* out, int size) {
-#pragma HLS INTERFACE m_axi port = in1 bundle = gmem0
-#pragma HLS INTERFACE m_axi port = in2 bundle = gmem1
-#pragma HLS INTERFACE m_axi port = out bundle = gmem0
+  tapa::mmap<hls::vector<uint32_t, NUM_WORDS>> in1,
+  tapa::mmap<hls::vector<uint32_t, NUM_WORDS>> in2,
+  tapa::mmap<hls::vector<uint32_t, NUM_WORDS>> out, int size) {
```

`tapa::mmap<T>` supports element-indexed reads and writes (`mem[i]`) just like a pointer, so the body of each task usually does not need to change.

### Step 3: Replace `hls::stream<T>&` with directional TAPA streams

Vitis HLS `hls::stream<T>&` is bidirectional — the same type is used whether the stream is read or written. TAPA makes direction explicit:

```diff
-void compute_add(hls::stream<hls::vector<uint32_t, NUM_WORDS>>& in1_stream,
-                 hls::stream<hls::vector<uint32_t, NUM_WORDS>>& in2_stream,
-                 hls::stream<hls::vector<uint32_t, NUM_WORDS>>& out_stream,
+void compute_add(tapa::istream<hls::vector<uint32_t, NUM_WORDS>>& in1_stream,
+                 tapa::istream<hls::vector<uint32_t, NUM_WORDS>>& in2_stream,
+                 tapa::ostream<hls::vector<uint32_t, NUM_WORDS>>& out_stream,
```

Use `tapa::istream<T>&` for streams the task reads from, and `tapa::ostream<T>&` for streams the task writes to. The `read()` and `<<` operators work the same as in Vitis HLS.

### Step 4: Replace local `hls::stream<T>` declarations

Local streams declared inside the top-level function become `tapa::stream<T>`:

```diff
-  hls::stream<hls::vector<uint32_t, NUM_WORDS>> in1_stream("input_stream_1");
-  hls::stream<hls::vector<uint32_t, NUM_WORDS>> in2_stream("input_stream_2");
-  hls::stream<hls::vector<uint32_t, NUM_WORDS>> out_stream("output_stream");
+  tapa::stream<hls::vector<uint32_t, NUM_WORDS>> in1_stream("input_stream_1");
+  tapa::stream<hls::vector<uint32_t, NUM_WORDS>> in2_stream("input_stream_2");
+  tapa::stream<hls::vector<uint32_t, NUM_WORDS>> out_stream("output_stream");
```

`tapa::stream<T>` accepts a name string for the same debugging purpose as `hls::stream<T>`. To set a custom depth, use `tapa::stream<T, DEPTH>`. For stream arrays, use `tapa::streams<T, ARRAY_SIZE, DEPTH>`.

```admonish note
The default stream depth in TAPA is 2, matching the Vitis HLS default. Unlike Vitis HLS, TAPA enforces the depth during software simulation, which helps catch backpressure bugs before synthesis.
```

### Step 5: Replace `#pragma HLS dataflow` with `tapa::task().invoke(...)`

Vitis HLS uses `#pragma HLS dataflow` to signal that a sequence of direct function calls should run as concurrent processes. TAPA replaces this with an explicit task graph:

```diff
-#pragma HLS dataflow
-  load_input(in1, in1_stream, size);
-  load_input(in2, in2_stream, size);
-  compute_add(in1_stream, in2_stream, out_stream, size);
-  store_result(out, out_stream, size);
+  tapa::task()
+      .invoke(load_input, in1, in1_stream, size)
+      .invoke(load_input, in2, in2_stream, size)
+      .invoke(compute_add, in1_stream, in2_stream, out_stream, size)
+      .invoke(store_result, out, out_stream, size);
```

All tasks in a `tapa::task().invoke(...)` chain run concurrently. The top-level function becomes pure orchestration — it declares streams, then hands everything off to child tasks.

---

## Example 2: Dataflow-in-a-loop

The full before and after files are at [example_2_before.cpp](code/vitis-hls/example_2_before.cpp) and [example_2_after.cpp](code/vitis-hls/example_2_after.cpp).

Vitis HLS permits `#pragma HLS dataflow` inside a for loop. Each iteration starts a new concurrent dataflow region:

```cpp
// Vitis HLS: dataflow region restarts each iteration
size /= NUM_WORDS;
for (int i = 0; i < size; i++) {
#pragma HLS dataflow
  load_input(in1, in1_stream, i);
  load_input(in2, in2_stream, i);
  compute_add(in1_stream, in2_stream, out_stream);
  store_result(out, out_stream, i);
}
```

TAPA does not allow computation in upper-level tasks. A top-level TAPA task may only declare streams and invoke child tasks — it cannot contain loops or arithmetic. The solution is to move the loop into each child task:

```cpp
// TAPA: loop lives in the child tasks
void load_input(tapa::mmap<hls::vector<uint32_t, NUM_WORDS>> in,
                tapa::ostream<hls::vector<uint32_t, NUM_WORDS>>& inStream,
                int size) {
  size /= NUM_WORDS;
  for (int i = 0; i < size; i++) {
#pragma HLS pipeline II = 1
    inStream << in[i];
  }
}
```

The top-level task then becomes:

```cpp
void vadd(...) {
  tapa::stream<...> in1_stream(...);
  tapa::stream<...> in2_stream(...);
  tapa::stream<...> out_stream(...);

  tapa::task()
      .invoke(load_input, in1, in1_stream, size)
      .invoke(load_input, in2, in2_stream, size)
      .invoke(compute_add, in1_stream, in2_stream, out_stream, size)
      .invoke(store_result, out, out_stream, size);
}
```

The child tasks stream data to each other for the full duration; no synchronization is needed between iterations because each task has its own loop that runs from start to finish.

---

## HLS-compat helpers for incremental migration

If you have a large existing codebase, TAPA provides `tapa::hls::stream<T>` as a drop-in replacement for `hls::stream<T>`. Unlike `tapa::stream<T>`, it uses effectively infinite depth in software simulation, so producers never block. This lets you keep direction-agnostic stream passing patterns while still running software simulation.

`tapa::hls::stream<T>` is available via `#include <tapa.h>` — no additional include is needed.

```cpp
// Before (Vitis HLS):
hls::stream<float>& s

// After (TAPA compat, passes software simulation without depth tuning):
tapa::hls::stream<float>& s
```

Use this as a stepping stone: get software simulation passing with `tapa::hls::stream`, then replace with directional `tapa::istream<T>&` / `tapa::ostream<T>&` before shipping.

```admonish note
`tapa::hls::stream` synthesizes correctly — the generated RTL FIFO is identical to `tapa::stream<T, N>`. The reason to replace it before hardware build is that the infinite simulation depth hides backpressure bugs. Switching to directional streams with a tuned depth catches those bugs during software simulation, before they appear on hardware.
```

---

**Next step:** [Lab 4: Custom RTL Modules](lab-04-custom-rtl.md)
