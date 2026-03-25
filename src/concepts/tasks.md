# Tasks

**Purpose:** Understand TAPA's three task types and their constraints.

**Prerequisites:** [The Programming Model](programming-model.md)

---

## Why this exists

TAPA organizes an FPGA accelerator as a hierarchy of C++ functions called
tasks. This hierarchy lets the compiler assign each leaf task to an independent
HLS module synthesized in parallel, while upper-level tasks provide the wiring
between those modules. The result is a design whose parallel structure is
explicit in the source code rather than inferred from pragmas.

---

## Mental model

A TAPA design forms a tree of tasks:

```
Top-level task (entry point, kernel boundary)
├── Upper-level task (orchestration only)
│   ├── Leaf task A (computation)
│   └── Leaf task B (computation)
└── Leaf task C (computation)
```

Each level has a distinct role:

- **Leaf task** — performs computation: loops, arithmetic, stream reads/writes.
  May call ordinary C++ functions. Must NOT invoke other TAPA tasks.
- **Upper-level task** — orchestrates execution. Its body may only instantiate
  streams and invoke child tasks with `tapa::task().invoke(...)`. It contains
  no computation of its own.
- **Top-level task** — the kernel entry point invoked from the host via
  `tapa::invoke`. For the `xilinx-vitis` target (the default), the top-level
  task must itself be an upper-level task.

---

## Minimal correct example

The `VecAdd` function from the vector-add example is a top-level upper-level
task. It instantiates three streams, then invokes four leaf tasks:

```cpp
void VecAdd(tapa::mmap<const float> a, tapa::mmap<const float> b,
            tapa::mmap<float> c, uint64_t n) {
  tapa::stream<float> a_q("a");
  tapa::stream<float> b_q("b");
  tapa::stream<float> c_q("c");

  tapa::task()
      .invoke(Mmap2Stream, a, n, a_q)
      .invoke(Mmap2Stream, b, n, b_q)
      .invoke(Add, a_q, b_q, c_q, n)
      .invoke(Stream2Mmap, c_q, c, n);
}
```

`Mmap2Stream`, `Add`, and `Stream2Mmap` are leaf tasks that each perform a
specific computation. `VecAdd` contains no computation — only stream
declarations and `.invoke(...)` calls.

---

## Detached tasks

By default a parent task waits for all child tasks to finish before it
terminates. A **detached** task is instead left running; the parent does not
wait for it. This is useful for purely data-driven tasks that have no natural
termination point (e.g., a constant data source or an infinite switch network).

```cpp
tapa::task().invoke<tapa::detach>(LeafTask, arg1, arg2);
```

Detached tasks are similar to `std::thread::detach` in the C++ STL. Because
their state does not need to be tracked, they avoid fan-out termination signals
and reduce area.

```admonish note
By default, TAPA tasks are joined: the parent waits for each child to complete.
Use `tapa::detach` only when the child task genuinely does not need to
terminate on program completion.
```

---

## Rules

- Leaf tasks receive streams by reference (`istream<T>&`, `ostream<T>&`) and
  mmap interfaces by value (`mmap<T>`).
- An upper-level task body must contain only stream instantiations and
  `.invoke(...)` calls — no loops, arithmetic, or other computation.
- `async_mmap` channel operations (`read_addr`, `read_data`, etc.) are
  leaf-task-only.
- For the `xilinx-vitis` target (the default), the top-level task must be an
  upper-level task — it cannot be a leaf task.
- Leaf templated tasks (template functions that compute directly) are
  supported. Non-leaf templated tasks that invoke other tasks are not yet
  supported.

---

## Common mistakes

**Wrong** — computation inside an upper-level task body:

```cpp
// Wrong: for loop makes this a leaf task, not an upper-level task
void BadUpper(tapa::mmap<float> mem, uint64_t n) {
  tapa::stream<float> q("q");
  for (uint64_t i = 0; i < n; ++i) {  // <-- computation here
    q.write(mem[i]);
  }
  tapa::task().invoke(Consumer, q, n);
}
```

**Right** — move computation into a dedicated leaf task:

```cpp
void Loader(tapa::mmap<float> mem, uint64_t n, tapa::ostream<float>& q) {
  for (uint64_t i = 0; i < n; ++i) {
    q.write(mem[i]);
  }
}

void GoodUpper(tapa::mmap<float> mem, uint64_t n) {
  tapa::stream<float> q("q");
  tapa::task()
      .invoke(Loader, mem, n, q)
      .invoke(Consumer, q, n);
}
```

---

## See also

- [Streams](streams.md)
- [Memory Access: async_mmap](async-mmap.md)
- [C++ API](../reference/api.md)

---

**Next step:** [Streams](streams.md)
