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
- **Upper-level task** — orchestrates execution. Its body may only declare
  channels and invoke child tasks with `tapa::task().invoke(...)`. It contains
  no computation of its own.
- **Top-level task** — the kernel entry point invoked from the host via
  `tapa::invoke`. For the `xilinx-vitis` target (the default), the top-level
  task must itself be an upper-level task.

---

## Minimal correct example

The `VecAdd` top-level task from [the programming
model](programming-model.md) is an upper-level task: it instantiates three
streams, then invokes four leaf tasks, and nothing else:

```cpp
  tapa::task()
      .invoke(Mmap2Stream, a, n, a_q)
      .invoke(Mmap2Stream, b, n, b_q)
      .invoke(Add, a_q, b_q, c_q, n)
      .invoke(Stream2Mmap, c_q, c, n);
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
and reduce area. The tasks a detached upper task invokes, joined or not, do not
hold up its parent either.

```admonish note
By default, TAPA tasks are joined: the parent waits for each child to complete.
Use `tapa::detach` only when the child task genuinely does not need to
terminate on program completion.
```

---

## Rules

- Leaf tasks receive streams by reference (`istream<T>&`, `ostream<T>&`) and
  mmap interfaces by value (`mmap<T>`).
- Every parameter of a task definition is a port named after the parameter,
  so `tapa analyze` rejects an unnamed one.
- A task built with HLS must not have a name beginning with `_`: Vitis HLS
  renames such a module, so `tapa analyze` rejects it.
- An upper-level task body must contain only channel declarations,
  constants, `assert`s, and one `tapa::task()` statement chaining every
  `.invoke(...)` — no loops, arithmetic, or other computation. `tapa analyze`
  rejects any other statement, and any invoke outside that one statement
  (in a second `tapa::task()` statement, a nested block, or a leaf task);
  hardware drops the `assert`s, as an `NDEBUG` build does.
- `async_mmap` channel operations (`read_addr`, `read_data`, etc.) are
  leaf-task-only.
- For the `xilinx-vitis` target (the default) and `xilinx-slash`, the
  top-level task must be an upper-level task — it cannot be a leaf task.
- Leaf templated tasks (template functions that compute directly) are
  supported. `tapa analyze` rejects a templated task that invokes other
  tasks.
- A task's RTL module takes the task's name, so a task cannot share a name
  with a Verilog module TAPA ships with the generated RTL (for example
  `fifo`, `arbiter`, `async_mmap`, `relay_station` or an
  `axi_crossbar_2x1` crossbar). `tapa analyze` rejects such a task and
  names the module.

---

## Common mistakes

**Wrong** — computation inside an upper-level task body:

```cpp
// Wrong: hardware keeps only the channels and invokes of an upper-level task,
// so the loop would never run there; `tapa analyze` rejects it
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
