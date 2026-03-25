# Streams

**Purpose:** Communicate between TAPA tasks using typed FIFO streams.

**Prerequisites:** [Tasks](tasks.md)

---

## Why this exists

Streams are the primary inter-task communication mechanism in TAPA. They are
typed, directional FIFOs that appear explicitly in task signatures, making data
flow visible in the source code. Unlike shared memory, streams enforce a
single-writer/single-reader discipline and make producer–consumer relationships
unambiguous to both the programmer and the compiler.

---

## Mental model

A stream instance lives in an upper-level task. Leaf tasks receive directional
references to it:

```cpp
// Upper-level task instantiates the stream and wires it to two leaf tasks
void Upper(/* ... */) {
  tapa::stream<float, 16> data_q("data_q");  // depth = 16 elements

  tapa::task()
      .invoke(Producer, data_q)   // Producer writes to data_q
      .invoke(Consumer, data_q);  // Consumer reads from data_q
}

// Leaf task signatures use directional references
void Producer(tapa::ostream<float>& out) { /* ... */ }
void Consumer(tapa::istream<float>& in)  { /* ... */ }
```

The `stream<T, Depth>` template parameter controls the hardware FIFO depth
(default: 2). A larger depth reduces the chance of stalls at the cost of FPGA
BRAM resources.

---

## Blocking read and write

```cpp
void Task(tapa::istream<int>& in, tapa::ostream<int>& out) {
  int data = in.read();   // blocks until data is available
  out.write(data);        // blocks until space is available
}
```

The `<<` and `>>` operator aliases are equivalent:

```cpp
out << data;   // same as out.write(data)
in >> data;    // same as data = in.read()
```

---

## Non-blocking read and write

To read from multiple streams or achieve an initiation interval of one, use the
non-blocking variants that return a `bool` indicating success:

```cpp
void Task(tapa::istream<int>& in, tapa::ostream<int>& out) {
  int data;
  bool ok = in.try_read(data);   // returns false if stream is empty
  if (ok) {
    out.try_write(data);         // returns false if stream is full
  }
}
```

---

## Readiness checks

Check stream state before committing to a read or write:

```cpp
if (!in.empty())  { /* safe to read  */ }
if (!out.full())  { /* safe to write */ }
```

For non-destructive inspection, `peek` returns the front element and a validity
flag without consuming it:

```cpp
bool valid;
auto val = in.peek(valid);   // does not remove the token
if (valid && /* routing decision */) {
  in.read(nullptr);          // consume now
}
```

---

## End-of-Transaction (EoT)

A producer signals the end of a data stream by calling `close()`. The consumer
detects it with `try_eot()`:

```cpp
// Producer
void Mmap2Stream(tapa::mmap<const float> mem, uint64_t n,
                 tapa::ostream<float>& stream) {
  for (uint64_t i = 0; i < n; ++i) {
    stream.write(mem[i]);
  }
  stream.close();  // send EoT token
}

// Consumer
void Stream2Mmap(tapa::istream<float>& stream, tapa::mmap<float> mem) {
  for (uint64_t i = 0;;) {
    bool eot;
    if (stream.try_eot(eot)) {
      if (eot) break;
      mem[i++] = stream.read(nullptr);
    }
  }
}
```

### EoT loop helper macros

TAPA provides macros that encapsulate the non-blocking EoT check pattern:

- `TAPA_WHILE_NOT_EOT(stream)` — loops until `stream` delivers an EoT token;
  body executes only when a valid non-EoT token is available.
- `TAPA_WHILE_NEITHER_EOT(s1, s2)` — loops until either stream delivers EoT;
  body executes only when both have a valid token.
- `TAPA_WHILE_NONE_EOT(s1, s2, s3)` — three-stream variant.

```cpp
void Consumer(tapa::istream<int>& in, tapa::ostream<int>& out) {
  TAPA_WHILE_NOT_EOT(in) {
    out.write(in.read(nullptr));
  }
  out.close();
}
```

```admonish tip
A downstream task can reopen a closed stream with `stream.open()` to reuse it
across multiple transactions.
```

---

## Stream arrays

For parameterized designs, TAPA provides arrays of streams:

- `tapa::streams<T, N>` — array of N streams (instantiation in upper-level task)
- `tapa::istreams<T, N>&` / `tapa::ostreams<T, N>&` — directional array
  references in leaf task signatures

When invoking `N` parallel instances of a leaf task, use `invoke<tag, N>(...)`
and TAPA distributes the array elements automatically:

```cpp
void InnerStage(int b, tapa::istreams<pkt_t, kN / 2>& in_q0,
                tapa::istreams<pkt_t, kN / 2>& in_q1,
                tapa::ostreams<pkt_t, kN> out_q) {
  tapa::task().invoke<tapa::detach, kN / 2>(Switch2x2, b, in_q0, in_q1, out_q);
}
```

---

## Rules

- Always pass streams by reference: `istream<T>&`, `ostream<T>&`. Never by
  value — the stream object is not copyable.
- Each stream instance must have exactly one reader and exactly one writer.
- TAPA software simulation respects stream depth: a full stream blocks the
  writer, matching hardware behavior.
- Stream depth is a hardware FIFO size. The FPGA resource used depends on depth:
  - **Depth ≤ 32**: synthesised from LUTs or SRL shift-registers (no BRAM cost).
  - **Depth 33–512**: mapped to BRAM (18K or 36K depending on width).
  - **Depth > 512**: may be mapped to URAM on devices that have it.
  - Default depth is 2, which costs only LUTs.

---

## Common mistakes

**Wrong** — stream passed by value (drops the reference, triggers a copy):

```cpp
void Leaf(tapa::istream<float> in) { /* ... */ }  // missing &
```

**Right** — stream passed by reference:

```cpp
void Leaf(tapa::istream<float>& in) { /* ... */ }
```

---

## See also

- [Tasks](tasks.md)
- [Deadlocks & Hangs](../troubleshoot/deadlocks-and-hangs.md)
- [C++ API](../reference/api.md)

---

**Next step:** [Memory Access: mmap](mmap.md)
