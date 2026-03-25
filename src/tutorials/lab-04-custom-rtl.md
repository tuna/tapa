# Lab 4: Custom RTL Modules

**Goal:** Replace a non-synthesizable TAPA task with a hand-written RTL module.

**Prerequisites:** [Lab 1: Vector Addition](lab-01-vadd.md) and familiarity with the TAPA compile pipeline.

After this lab you will understand how to label unsynthesizable tasks, generate RTL port templates, provide custom RTL implementations, and repack them into a deployable XO.

---

## When to use this

Use custom RTL modules when:

- A task uses dynamic memory allocation (`new`/`delete`, `malloc`/`free`), which TAPA HLS cannot synthesize.
- An existing RTL implementation is available from a vendor IP or a prior design, and reimplementing it in HLS would be wasteful.
- A task is too complex to express in C++ HLS and a direct RTL description is more practical.

---

## Step 1: Label the non-synthesizable task

Given a design where `Add` allocates memory dynamically, the `Add` task itself is unsynthesizable. Wrap it in an upper-level task and annotate that wrapper with `[[tapa::target("ignore")]]`:

```cpp
[[tapa::target("ignore")]] void Add_Upper(
    tapa::istream<float>& a, tapa::istream<float>& b,
    tapa::ostream<float>& c, uint64_t n) {
  tapa::task().invoke(Add, a, b, c, n);
}
```

The attribute tells TAPA to skip synthesis of `Add_Upper` and to generate RTL port template files instead. The rest of the design compiles normally.

---

## Step 2: Compile to generate template files

Compile the design as usual:

```bash
tapa compile \
  --top VecAdd \
  --part-num xcu250-figd2104-2L-e \
  --clock-period 3.33 \
  -f vadd.cpp \
  -o work.out/vadd.xo
```

Because `Add_Upper` is tagged `ignore`, TAPA generates RTL template files under `work.out/template/`. These templates define the exact port signatures the replacement RTL module must match.

---

## Step 3: Implement the RTL

Write or adapt your custom RTL files so their port declarations match the generated templates. When you run `tapa pack --custom-rtl` (or `tapa compile --custom-rtl`) in Step 4, TAPA performs advisory port checking on `.v` files: it warns on mismatches but does not abort the build. Resolve any reported mismatches before moving to hardware.

---

## Step 4: Repack with custom RTL

Two workflows are available depending on whether you are iterating on the RTL separately from the HLS compilation step.

**Option A — Two-step workflow** (compile once, iterate on RTL separately):

```bash
tapa pack \
  -o work.out/vadd.xo \
  --custom-rtl ./rtl/
```

**Option B — One-step workflow** (compile and pack together):

```bash
tapa compile \
  --top VecAdd \
  --part-num xcu250-figd2104-2L-e \
  --clock-period 3.33 \
  -f vadd.cpp \
  -o work.out/vadd.xo \
  --custom-rtl ./rtl/
```

`--custom-rtl` accepts a file path or a directory. To include multiple paths, repeat the flag. `.v` files receive advisory port checking; other file types (for example `.tcl`) are packaged without format checking.

---

## Validation

After repacking, run fast cosim to verify the custom RTL produces correct results before committing to a full bitstream build:

```bash
./vadd --bitstream=work.out/vadd.xo 1000
```

Catching functional bugs at cosim time is far cheaper than discovering them after hours of bitstream generation.

---

## Full example

The complete working example is in `tests/functional/custom-rtl` in the TAPA repository.

---

**Next step:** [Lab 5: Floorplan & DSE](lab-05-floorplan.md)
