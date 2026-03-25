# Lab 4: Custom RTL Modules

**Goal:** Replace a TAPA task with a hand-written RTL module while keeping a C++ behavior model for software simulation.

**Prerequisites:** [Lab 1: Vector Addition](lab-01-vadd.md) and familiarity with the TAPA compile pipeline.

After this lab you will understand how to write a C++ behavior model for an ignored task, label it for RTL replacement, generate RTL port templates, provide custom RTL, and repack into a deployable XO.

---

## When to use this

Use custom RTL modules when:

- An existing RTL implementation is available from a vendor IP catalog or a prior design, and reimplementing it in HLS would be wasteful.
- A task requires timing, area, or interface characteristics that HLS cannot produce.
- A task is too complex to express in synthesizable C++ and a direct RTL description is more practical.

---

## Overview

The workflow has three parts:

1. Write a **C++ behavior model** that correctly implements the task — this is what runs during software simulation. The code does not need to be synthesizable.
2. Wrap the behavior model in a task annotated with `[[tapa::target("ignore")]]`. TAPA compiles the rest of the design normally and generates RTL port template files for the ignored task instead of synthesizing it.
3. Provide the actual RTL implementation and repack the XO.

---

## Example: using a vendor floating-point IP

Suppose you have a task that computes element-wise reciprocal square root and want to use Xilinx's Floating-Point IP core rather than the HLS-generated logic.

### Step 1: Write the C++ behavior model

The behavior model lives in an ordinary task function. It will be called during software simulation and will never be synthesized, so it can use any C++ — standard library calls, dynamic containers, whatever is convenient and correct.

```cpp
#include <cmath>
#include <tapa.h>

// Behavior model: runs during software simulation only.
// Uses std::sqrt — this does not need to be synthesizable.
void RsqrtCore(tapa::istream<float>& in, tapa::ostream<float>& out,
               uint64_t n) {
  for (uint64_t i = 0; i < n; ++i) {
    float val = in.read();
    out.write(1.0f / std::sqrt(val));  // stdlib call: fine for simulation
  }
}
```

### Step 2: Wrap with `[[tapa::target("ignore")]]`

Create a thin wrapper that invokes the behavior model. The `[[tapa::target("ignore")]]` attribute tells TAPA to skip synthesis of this wrapper and generate RTL port templates in its place. During software simulation the wrapper runs normally, which in turn calls `RsqrtCore`.

```cpp
[[tapa::target("ignore")]] void Rsqrt(
    tapa::istream<float>& in, tapa::ostream<float>& out, uint64_t n) {
  tapa::task().invoke(RsqrtCore, in, out, n);
}
```

```admonish note
Only the **wrapper** needs the attribute. The behavior model (`RsqrtCore`) is a plain task function. Software simulation runs the wrapper as usual; synthesis skips it and generates port templates.
```

### Step 3: Integrate into the top-level task

```cpp
void Pipeline(tapa::mmap<const float> in, tapa::mmap<float> out, uint64_t n) {
  tapa::stream<float> in_q("in");
  tapa::stream<float> out_q("out");

  tapa::task()
      .invoke(Load, in, n, in_q)
      .invoke(Rsqrt, in_q, out_q, n)   // custom RTL replaces this
      .invoke(Store, out_q, out, n);
}
```

### Step 4: Compile to generate template files

```bash
tapa compile \
  --top Pipeline \
  --part-num xcu250-figd2104-2L-e \
  --clock-period 3.33 \
  -f pipeline.cpp \
  -o work.out/pipeline.xo
```

Because `Rsqrt` is tagged `ignore`, TAPA generates RTL template files under `work.out/template/`. These templates define the exact port signatures the replacement RTL module must match.

### Step 5: Implement the RTL

Write or adapt your RTL files so their port declarations match the generated templates. When you run `tapa pack --custom-rtl` in the next step, TAPA performs advisory port checking on `.v` files: it warns on mismatches but does not abort the build. Resolve any reported mismatches before moving to hardware.

### Step 6: Repack with custom RTL

Two workflows are available depending on whether you are iterating on the RTL separately from the HLS compilation step.

**Option A — Two-step workflow** (compile once, iterate on RTL separately):

```bash
tapa pack \
  -o work.out/pipeline.xo \
  --custom-rtl ./rtl/
```

**Option B — One-step workflow** (compile and pack together):

```bash
tapa compile \
  --top Pipeline \
  --part-num xcu250-figd2104-2L-e \
  --clock-period 3.33 \
  -f pipeline.cpp \
  -o work.out/pipeline.xo \
  --custom-rtl ./rtl/
```

`--custom-rtl` accepts a file path or a directory. To include multiple paths, repeat the flag. `.v` files receive advisory port checking; other file types (for example `.tcl`) are packaged without format checking.

---

## Software simulation with the behavior model

Because the behavior model is plain C++, software simulation works exactly as for any other TAPA design:

```bash
tapa g++ -- pipeline.cpp host.cpp -o pipeline
./pipeline
```

The behavior model does not need to match the RTL cycle-accurately — it only needs to produce the correct output values. Use this to validate host logic and data paths before RTL is ready.

```admonish note
The behavior model code can freely use unsynthesizable constructs: standard library functions, dynamic allocation, floating-point math, file I/O for golden output comparison, and so on. TAPA never attempts to synthesize it.
```

---

## Validation

After repacking, run fast cosim to verify the custom RTL produces correct results before committing to a full bitstream build:

```bash
./pipeline --bitstream=work.out/pipeline.xo 1000
```

Catching functional bugs at cosim time is far cheaper than discovering them after hours of bitstream generation.

---

## Full example

The complete working example is in `tests/functional/custom-rtl` in the TAPA repository.

---

**Next step:** [Lab 5: Floorplan & DSE](lab-05-floorplan.md)
