# Common Errors

Symptom descriptions and fixes for the most common compile-time and runtime errors.

**When to use this page:** When `tapa g++` or `tapa compile` reports an error, or when software simulation crashes or produces wrong output.

---

## Stream passed by value

**Symptom:** Compile error mentioning a deleted copy constructor, or that `istream`/`ostream` is not CopyConstructible.

**Cause:** The stream parameter is declared without `&`. Streams are non-copyable objects — they represent live communication channels between tasks, not data values.

**Fix:** Always pass streams by reference.

```cpp
// Wrong
void Task(tapa::istream<int> in, tapa::ostream<int> out) { ... }

// Right
void Task(tapa::istream<int>& in, tapa::ostream<int>& out) { ... }
```

---

## `mmap` passed by reference

**Symptom:** Compile error about a type mismatch or an unexpected `&` on an `mmap` parameter.

**Cause:** `tapa::mmap<T>` is essentially a pointer to a memory region and must be passed by value, not by reference.

**Fix:** Remove the `&` from `mmap` parameters.

```cpp
// Wrong
void Task(tapa::mmap<int>& mem) { ... }

// Right
void Task(tapa::mmap<int> mem) { ... }
```

---

## `async_mmap` passed by value

**Symptom:** Passing `async_mmap` by value is deprecated and may produce a warning or error depending on the TAPA version.

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

**Symptom:** `tapacc` reports an error about computation in an upper-level task, or the design fails synthesis unexpectedly.

**Cause:** Upper-level tasks (tasks that invoke other tasks) may only contain stream declarations and `.invoke()` chains. Any arithmetic, conditionals, or other function calls belong in leaf tasks. For example, computing `n * 2` directly in `TopLevel` is not allowed:

```cpp
// Wrong
void TopLevel(int n, tapa::mmap<int> mem) {
  tapa::stream<int> s("s");
  tapa::task()
    .invoke(Task1, s, mem, n * 2)
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

**Cause:** `tapa::stream<T> arr[N]` is not copyable or movable in the way TAPA expects. Arrays of streams must use the dedicated `tapa::streams<T, N>` type.

**Fix:** Use `tapa::streams<T, N>` for stream arrays, and use `.invoke` with a count to distribute elements rather than indexing manually.

```cpp
// Wrong
tapa::stream<int> data_q[4];
tapa::task().invoke(Task, data_q[0], mem[0])
            .invoke(Task, data_q[1], mem[1]);

// Right
tapa::streams<int, 4> data_q;
tapa::mmaps<int, 4> mem;
tapa::task().invoke<tapa::join, 4>(Task, data_q, mem);
```

---

## `tapac` not found

**Symptom:** Shell reports `command not found: tapac`.

**Cause:** `tapac` was the old command name. It has been replaced by `tapa compile`.

**Fix:** Replace `tapac` with `tapa compile`. Most flags carry over directly.

```bash
# Old
tapac --top VecAdd -f vadd.cpp -o vadd.xo ...

# New
tapa compile --top VecAdd -f vadd.cpp -o vadd.xo ...
```

Run `tapa compile --help` for the full option list.

---

## Tasks not defined in the same compilation unit as the top-level function

**Symptom:** `tapacc` cannot find a task function, or a link error occurs for a task symbol.

**Cause:** TAPA requires all task functions to be visible in the same compilation unit as the top-level function. Placing tasks in separate `.cpp` files means the compiler never sees them together.

**Fix:** Define tasks in header files and `#include` them in the main kernel file.

```cpp
// task1.hpp
void Task1(/* ... */) { /* ... */ }

// task2.hpp
void Task2(/* ... */) { /* ... */ }

// top_level.cpp
#include "task1.hpp"
#include "task2.hpp"

void TopLevel(/* ... */) {
  tapa::task().invoke(Task1, /* ... */).invoke(Task2, /* ... */);
}
```

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

**Fix:** Avoid static variables inside tasks. Pass state between tasks using stream or mmap arguments.

---

```admonish tip
If a parameter type mismatch error is confusing, work through this checklist:
1. Does the number of arguments at the call site match the task signature?
2. Are stream directions correct — `istream` for reads, `ostream` for writes?
3. Are passing conventions correct — streams and `async_mmap` by reference, `mmap` by value?
4. Is the parameter order the same between the call site and the task definition?
```

---

**See also:** [Deadlocks & Hangs](deadlocks-and-hangs.md) | [Cosimulation Issues](cosim-issues.md)
