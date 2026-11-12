# Common Errors

Symptom descriptions and fixes for the most common compile-time and runtime errors.

**When to use this page:** When `tapa g++` or `tapa compile` reports an error, or when software simulation crashes or produces wrong output.

---

## Stream passed by value

**Symptom:** `tapacc` reports `tapa::istream parameter 'in' must be passed by reference` (or `tapa::ostream`), and `tapa g++` stops at a `static_assert`: `tapa::istream<T>& must be passed by reference as a TAPA task parameter`.

**Cause:** The stream parameter is declared without `&`. A TAPA task port
must preserve the endpoint's reference identity; passing it by value is
rejected.

**Fix:** Always pass streams by reference.

```cpp
// Wrong
void Task(tapa::istream<int> in, tapa::ostream<int> out) { ... }

// Right
void Task(tapa::istream<int>& in, tapa::ostream<int>& out) { ... }
```

---

## `mmap` passed by reference

**Symptom:** `tapacc` reports `tapa::mmap parameter 'mem' must be passed by value (not by reference)`, and `tapa g++` stops at a `static_assert`: `tapa::mmap<T> must be passed by value as a TAPA task parameter`.

**Cause:** `tapa::mmap<T>` is essentially a pointer to a memory region and must be passed by value, not by reference.

**Fix:** Remove the `&` from `mmap` parameters.

```cpp
// Wrong
void Task(tapa::mmap<int>& mem) { ... }

// Right
void Task(tapa::mmap<int> mem) { ... }
```

---

## Raw pointer used as a task memory port

**Symptom:** `tapacc` reports that a raw pointer task parameter is not a TAPA
memory port.

**Cause:** Vitis HLS uses raw pointer parameters plus `m_axi` pragmas for
off-chip memory. TAPA task ports encode that interface in the C++ type itself,
so a raw pointer is not a valid task memory port.

**Fix:** Replace the raw pointer with `tapa::mmap<T>` passed by value. No
`m_axi` pragma is needed.

```cpp
// Wrong
void Load(const int* mem, tapa::ostream<int>& out) { /* ... */ }

// Right
void Load(tapa::mmap<const int> mem, tapa::ostream<int>& out) { /* ... */ }
```

---

## `async_mmap` passed by value

**Symptom:** `tapacc` reports `tapa::async_mmap parameter 'mem' must be passed by reference`, and `tapa g++` stops at `tapa::async_mmap<T>& must be passed by reference as a TAPA task parameter`.

**Cause:** `tapa::async_mmap<T>` is a set of streams that controls memory access. Like regular streams, it must be passed by reference.

**Fix:** Always pass `async_mmap` by reference.

```cpp
// Wrong
void Task(tapa::async_mmap<int> mem) { ... }

// Right
void Task(tapa::async_mmap<int>& mem) { ... }
```

---

## Computation in upper-level task body

**Symptom:** `tapacc` reports `upper task '<name>' computes outside its invokes; hardware keeps only its channel declarations and invokes, so move this into a leaf task`, or, for computation inside an invoke argument, `cannot bind a <Clang expression kind> here; each invoke argument after the task must be a stream or mmap channel, a scalar naming a parent task parameter, or an integer constant`.

**Cause:** Upper-level tasks (tasks that invoke other tasks) may only contain channel declarations, constants, `assert`s, and `.invoke()` chains. Any arithmetic, conditionals, or other function calls belong in leaf tasks. For example, computing `n * 2` directly in `TopLevel` is not allowed:

```cpp
// Wrong
void TopLevel(int n, tapa::mmap<int> mem) {
  tapa::stream<int> s("s");
  tapa::task()
    .invoke(Task1, s, mem, n)
    .invoke(Task2, s, n * 2);
}
```

**Fix:** Move the computation into the child task that uses the result.

```cpp
// Right
void Task2(tapa::istream<int>& in, int n) {
  n = n * 2;
  // use n ...
}

void TopLevel(int n, tapa::mmap<int> mem) {
  tapa::stream<int> s("s");
  tapa::task()
    .invoke(Task1, s, mem, n)
    .invoke(Task2, s, n);
}
```

---

## Stream array declared as `stream[]` instead of `streams<>`

**Symptom:** Compile error or incorrect behavior when defining or passing arrays of streams.

**Cause:** A raw `tapa::stream<T> arr[N]` does not provide TAPA's stream-array
access and distribution machinery. Use the dedicated `tapa::streams<T, N>`
type when passing indexed stream arrays to task invocations.

**Fix:** Use `tapa::streams<T, N>` for stream arrays, and use `.invoke` with a count to distribute elements rather than indexing manually.

```cpp
// Wrong
tapa::stream<int> data_q[4];
tapa::task().invoke(Task, data_q[0], mem[0])
            .invoke(Task, data_q[1], mem[1]);

// Right
void Top(tapa::mmaps<int, 4> mem) {
  tapa::streams<int, 4> data_q;
  tapa::task().invoke<tapa::join, 4>(Task, data_q, mem);
}
```

---

## Task has no definition in any scanned translation unit

**Symptom:** `tapacc` reports `task 'X' is invoked by 'Y' at <file>:<line> but has no definition in any scanned translation unit (…)`, listing the inputs it scanned.

**Cause:** The translation unit that defines the task was not passed to `tapa analyze`. If a function with the same name but a different signature exists, the message points at it — the declaration and the definition must match.

**Fix:** Pass every translation unit of the program, one per `-f`. A single `tapacc` process parses all inputs with the same cflags, so a task may be defined in any input file (or a header) and invoked from any other. See
[Multi-File Designs](../howto/multi-file.md) for the complete workflow.

---

## Task builds a different task graph in two translation units

**Symptom:** `tapacc` reports `task 'X' builds a different task graph in two translation units: … invokes 'A' but … invokes 'B'; a task sighted in several translation units must build the same task graph in each`.

**Cause:** A task definition shared by several translation units — usually in a header every input includes — invokes different tasks in each, typically because a macro each translation unit defines differently selects the invoked task. The merged program keeps exactly one graph per task, so both variants cannot be honored; an identical shared definition is required, not just an identical signature.

**Fix:** Make the shared definition build the same graph wherever it is sighted: define the selecting macro to the same value before the header is included in every translation unit, or hoist the selection out of the shared body — give each variant its own task and choose between them at the parent's invoke site.

---

## Task with internal linkage

**Symptom:** `tapacc` reports that a task invoked by another `is not an externally linked free function (static, anonymous namespace, or a member function); tasks must be`, or that a function `builds a tapa::task() but is not an externally linked global function (static, anonymous namespace, or a member function), so it can never be a task`.

**Cause:** A task is an RTL module instantiated by its parent, referenced across translation units. A `static` function, a function in an anonymous namespace, or a member function (a `static` one included) has internal or class linkage and can never fill that role.

**Fix:** Give the task external linkage: move it out of the anonymous namespace, drop the `static`, or make it a free function.

---

## `ap_wait` is not declared in software simulation

**Symptom:** The host build fails with an undeclared `ap_wait` or `ap_wait_n`,
although Vitis HLS accepts the kernel.

**Cause:** These are vendor synthesis intrinsics that impose cycle boundaries
in generated RTL. TAPA software simulation is transaction-based and does not
provide a cycle-accurate host implementation.

**Fix:** Use the portable form: `tapa::wait()` (or `tapa::wait(n)`) is a
no-op in software simulation and lowers to `ap_wait`/`ap_wait_n` when
synthesizing, so the kernel builds on both paths unchanged. If you must keep
the vendor intrinsic (e.g. in code shared with a non-TAPA build), guard it
explicitly:

```cpp
#ifdef __SYNTHESIS__
  ap_wait();
#endif
```

Do not use the guard to hide synchronization required by the program's dataflow
semantics. If another task must observe a phase boundary, express it with a
stream token or another explicit handshake so software simulation and RTL obey
the same contract.

---

## Static variables behave differently in simulation vs hardware

**Symptom:** Software simulation produces different output than hardware execution.

**Cause:** Static variables are shared across all invocations within a single simulation process. In hardware, each task instance synthesizes its own independent copy of the variable.

For example:

```cpp
void Task() {
  static int counter = 0;
  counter++;
}

tapa::task().invoke(Task).invoke(Task);
```

In software simulation `counter` reaches 2 (one shared variable, incremented twice). In hardware each instance has its own `counter`, so both instances end at 1.

**Fix:** Avoid function-static variables inside tasks. Use automatic state
where possible, or pass explicit state through streams or mmaps so each task
instance has well-defined behavior.

---

```admonish tip
If a parameter type mismatch error is confusing, work through this checklist:
1. Does the number of arguments at the call site match the task signature?
2. Are stream directions correct — `istream` for reads, `ostream` for writes?
3. Are passing conventions correct — streams and `async_mmap` by reference, `mmap` by value?
4. Is the parameter order the same between the call site and the task definition?
```

---

## `v++ 60-1299`: platform not supported

**Symptom:** `v++ --link` fails with
`ERROR: [v++ 60-1299] The specified platform is not supported. Platform '<name>.xpfm' (version <year>) is not supported by the current tool version`.

**Cause:** The installed `v++` does not support the selected platform. TAPA
can still read the platform and synthesize for its FPGA part, so the failure
may not appear until `v++ --link`.

**Fix:** Choose a platform supported by both your Vitis installation and your
target board, then use that same platform for `tapa compile` and `v++ --link`.
Do not switch to a platform for a different board merely to bypass the
error.

---

## `v++ 17-1396`: XO container failed integrity check

**Symptom:** `tapa link` (or `v++ --link` on a TAPA `.xo`) logs
`CRITICAL WARNING: [v++ 17-1396] The specified XO container failed integrity check: '<name>.xo'`,
and the link then continues.

**Cause:** Vivado's `package_xo` signs the archive it writes, in its ZIP
comment, over the archive's exact bytes. `tapa pack` then rewrites the `.xo`:
it removes timestamps, absolute paths and random IDs so that repeated builds
are byte-identical, and adds `tapa.json` and the HLS reports, which `tapa
link` and fast cosimulation read. Any rewrite, even one that keeps every
entry's content, invalidates the signature, so every TAPA `.xo` triggers
this warning.

**Fix:** None is needed. The warning is harmless: `v++` links the `.xo`
normally, and the kernel it contains is the one Vivado packaged.

---

**See also:** [Deadlocks & Hangs](deadlocks-and-hangs.md) | [Cosimulation Issues](cosim-issues.md)
