# Multi-File Designs

TAPA can analyze a kernel whose tasks are defined in different C++ translation
units. Pass every kernel source in one compilation, keep task declarations
visible where they are invoked, and use the same compiler flags for all kernel
sources.

The working example used below is in `tests/functional/multi-file/`.

## Source organization

The example has four files:

- `multi-file.h` declares the top and leaf tasks, defines a leaf template task,
  and declares an ordinary helper function.
- `a.cpp` defines the top task, one leaf task, and the helper.
- `b.cpp` defines another leaf task invoked by the top in `a.cpp`.
- `multi-file-host.cpp` calls the top with `tapa::invoke`.

The shared header makes every required declaration visible to both kernel
translation units:

```cpp
#pragma once
#include <cstdint>
#include <tapa.h>

void Produce(tapa::mmap<const float> a, tapa::mmap<const float> b, uint64_t n,
             tapa::ostream<float>& a_q, tapa::ostream<float>& b_q);

void Consume(tapa::istream<float>& c_q, tapa::mmap<float> c, uint64_t n);

void MultiFileTop(tapa::mmap<const float> a, tapa::mmap<const float> b,
                  tapa::mmap<float> c, uint64_t n);

template <typename T>
void Combine(tapa::istream<T>& a_q, tapa::istream<T>& b_q,
             tapa::ostream<T>& c_q, uint64_t n) {
  for (uint64_t i = 0; i < n; ++i) {
    c_q << (a_q.read() + b_q.read());
  }
}
```

`a.cpp` and `b.cpp` both include this header. The top in `a.cpp` can therefore
invoke `Consume` even though its definition is in `b.cpp`. A declaration must
be visible at the invocation site, and the corresponding definition must be in
one of the sources passed to TAPA.

Template tasks follow the usual C++ rule: place their definitions in a header
so the required instantiations are visible. TAPA supports leaf template tasks
and explicit specializations (`template <>`) of leaf and non-leaf templates;
a non-leaf task instantiated from a template is not supported. An explicit
specialization is defined once, like any function, and declared where it is
invoked.

## Build for software simulation

From `tests/functional/multi-file/`, build all kernel sources and the host in
one command:

```bash
tapa g++ -- \
  a.cpp b.cpp multi-file-host.cpp \
  -I../multi-file-ext \
  -o multi-file

./multi-file
```

`-I../multi-file-ext` finds the additional `helper.h` used by both kernel
sources. Headers are included normally; do not pass them with `-f` or as
separate host inputs. A successful run prints `PASS!`.

## Analyze and compile the kernel

For analysis only, repeat `-f` for every translation unit and `-c` for every
shared compiler flag:

```bash
tapa --work-dir work.out analyze \
  --top MultiFileTop \
  -f a.cpp \
  -f b.cpp \
  -c -I../multi-file-ext
```

For a complete build, use the same source list with `tapa compile`:

```bash
tapa --work-dir work.out compile \
  --top MultiFileTop \
  --part-num xcu250-figd2104-2L-e \
  --clock-period 3.33 \
  -f a.cpp \
  -f b.cpp \
  -c -I../multi-file-ext \
  -o multi-file.xo
```

All `-f` sources form one task graph. Do not run `tapa analyze` separately for
each source: that creates separate compiler states rather than combining the
files. Likewise, compiler flags are shared across all sources; there is no
per-file `-c` syntax.

## Rules for multi-file programs

- Select one top task with `--top`. Other files can contain leaf tasks and
  ordinary helper functions.
- Declare tasks before invoking them, usually in a shared header, and include
  every definition with `-f`.
- Tasks must be externally linked free functions. Do not declare a task
  `static`, put it in an anonymous namespace, or make it a member function.
- A task definition shared by multiple translation units must be identical
  everywhere and build the same task graph. In particular, do not use a
  per-file macro to make a shared upper task invoke different children.
- Ordinary helpers also need a visible declaration at each call site. Define
  non-inline helpers once; define inline helpers in a shared header.
- Use `-c` for include paths and other flags shared by every kernel source.
  Use the same include paths when building the host.

## Common errors

Multi-file builds most often fail with *task has no definition in any
scanned translation unit* (a defining source was not passed with `-f`) or
*task builds a different task graph in two translation units* (a shared
definition varies between them); a missing `-I` flag shows up as *header not
found*. See [Common Errors](../troubleshoot/common-errors.md) for the full
diagnostics and fixes.
