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

An inter-task stream instance lives in an upper-level task. Leaf tasks receive
directional references to it:

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
(default: 2). A larger depth can reduce stalls at the cost of additional FPGA
resources.

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

Non-blocking variants report whether they transferred a token. Always check
the result: a failed read consumes nothing, and a failed write sends nothing.

```cpp
int next_value = 42;
if (out.try_write(next_value)) {
  // next_value was written; compute the next token.
}

int data;
if (in.try_read(data)) {
  // data was consumed; keep it until every later write succeeds.
}
```

If a consumer must forward data through a full output stream, retain the value
and retry the write before reading another input token.

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

A producer marks the end of a transaction by calling `close()`. The consumer
detects the marker with `try_eot()`, which inspects but does not consume it:

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
  stream.open();  // consume the EoT marker before the next transaction
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
  in.open();
  out.close();
}
```

```admonish tip
After observing EoT, the consumer calls `stream.open()` to consume the marker.
`close()` and `open()` delimit transactions; they do not destroy and recreate
the stream. Consume the marker before starting the next transaction or ending
the task.
```

---

## Stream arrays

For parameterized designs, TAPA provides arrays of streams:

- `tapa::streams<T, N, Depth>` — array of N streams (instantiation in an
  upper-level task; `Depth` defaults to 2)
- `tapa::istreams<T, N>&` / `tapa::ostreams<T, N>&` — directional array
  references in leaf task signatures

When invoking `N` parallel instances of a leaf task, use `invoke<tag, N>(...)`
and TAPA distributes the array elements automatically:

```cpp
void InnerStage(int b, tapa::istreams<pkt_t, kN / 2>& in_q0,
                tapa::istreams<pkt_t, kN / 2>& in_q1,
                tapa::ostreams<pkt_t, kN>& out_q) {
  tapa::task().invoke<tapa::detach, kN / 2>(Switch2x2, b, in_q0, in_q1, out_q);
}
```

---

## Frame ports

A stream can also move whole frames. A leaf task takes its end as a frame
port, `tapa::iframes<T, N>&` or `tapa::oframes<T, N>&`, and works on one
frame of `N` elements at a time, in any order, through the view `acquire()`
returns:

```cpp
void Corner(tapa::iframes<float, 1024>& in, tapa::oframes<float, 1024>& out,
            int n) {
  for (int f = 0; f < n; ++f) {
    auto src = in.acquire();  // the next whole frame, read-only
    auto dst = out.acquire();  // an empty frame, write-only
    for (int r = 0; r < 32; ++r) {
      [[tapa::pipeline(1)]] for (int c = 0; c < 32; ++c) {
        dst[c * 32 + r] = src[r * 32 + c];
      }
    }
  }  // both released at the end of their scope, or earlier by release()
}
```

The other end of the stream may be a frame port or an ordinary `istream` /
`ostream`, which sees the frames' elements in order. The stream is still a
`tapa::stream<T, Depth>`, or a stream port the invoking upper task takes and
hands on, up to the stream's declaration or the kernel's own stream
argument.

- **On PL** the stream becomes a frame channel: `max(2, Depth / N)` banks of
  `N` elements in one memory, handed between the two ends in turn. A frame
  port reaches its task as a memory port of the current bank, so a frame is
  read or written in place without a copy, and the producer fills one bank
  while the consumer works on another. The channel sits where the stream is
  declared; an upper task in between carries the frame port's signals in
  place of the stream's. A stream to the AI Engine gets a channel of two
  banks in the PL kernel's top, filled from or drained to its PLIO element
  by element; so does a stream argument of the kernel itself, which on
  Vitis comes from the host as described below. When `tapa floorplan`
  puts the two ends in different slots, the channel is copied, as an AI
  Engine tile DMA copies a buffer between distant tiles: each frame-port end
  gets its own channel of the same banks, and the elements cross the slots
  through the stream's pipeline. A copy costs the extra banks and delays the
  consumer by a frame; ends in one slot share one channel.
- **On the AI Engine** a frame port is the kernel's buffer port, filled or
  drained by DMA. Between two AI Engine frame ports the graph connects the
  buffers directly, so neighbouring kernels share tile memory.
- **In software simulation** `acquire()` copies the frame out of the stream
  (input) or into a buffer the release writes to the stream (output), so the
  stream holds up to its depth besides the frame each end holds.

Rules:

- A port holds one view at a time: release the frame before acquiring the
  next.
- A frame port's stream carries no end-of-transmission token; loop over a
  frame count.
- Frames on one port never overlap in time. In HLS the loop around an
  `acquire()` is not pipelined; pipeline the loops inside it.
- Both ends of a stream that move frames agree on `N`, and its depth holds
  at least one frame (`tapa analyze` checks both).
- A top task that is itself a leaf cannot take a frame port: nothing would
  hold its channel. Invoke the leaf from an upper top task.

### Frames from and to the host

XRT has no streams between host and kernel, so on Vitis a stream argument
of the top that reaches a frame port, or a leaf's `[[tapa::frame(N)]]`
stream port, moves through device memory: a ring of frame slots, 4 unless
the top's parameter says otherwise,
`[[tapa::ring(S)]] tapa::istream<float>& rows`. The host program is the
same as in software simulation: it passes a `tapa::stream`, and TAPA's
runtime moves each frame between it and a free slot while the kernel runs.
In the kernel a mover streams each frame between its slot and the
argument's frame channel, or for a `[[tapa::frame(N)]]` stream port on PL
an ordinary FIFO; for an output, the kernel is done only once everything
it released into that channel (every frame, or every element written into
the FIFO) is in memory.

- Every Vitis flow runs the ring: xosim, Verilator cosimulation, hardware
  emulation and hardware. A stream argument that reaches neither stays an
  AXIS port, which only simulation can feed.
- An AI Engine top with no PL task gets the same rings on a VCK5000, in a
  PL kernel TAPA builds for it ([AI Engine
  Tasks](../howto/aie.md#on-a-vck5000)).
- In `kernel.xml` the argument is a memory argument (its own `m_axi` port,
  placed by connectivity like any other), followed after every argument
  of the top by three registers per ring: the host's and the kernel's frame
  counts, and the ring's shape.
- Per frame the host pays a DMA transfer and a few register accesses; a
  larger frame, or more slots, amortizes them. In hardware emulation each
  of them waits behind XRT's own polling of the running kernel, so a few
  frames there take minutes, at times over an hour.
- The host reads the kernel's registers during the run, so XRT opens the
  kernel for exclusive access: one process at a time.
- An input frame the kernel has not taken when an invoke ends carries into
  the next invoke on XRT, as in a stream; cosimulation starts each invoke
  afresh, so there it is dropped.
- `[[tapa::ring(S)]]` belongs on such a stream parameter of the top only;
  anywhere else it is an error, as is an `S` of 0.

---

## Rules

- Always pass TAPA task stream ports by reference: `istream<T>&`,
  `ostream<T>&`, `iframes<T, N>&`, `oframes<T, N>&`. Passing one by value is
  rejected.
- A TAPA task port uses `istream<T>&` or `ostream<T>&`, not `stream<T>`.
  `stream` declares a channel; an upper-level task declares inter-task channels
  in its body. A top-level kernel likewise exposes external stream ports as
  `istream`/`ostream`.
- Each stream instance must have exactly one reader and exactly one writer.
- With `tapa::stream<T, Depth>`, software simulation enforces `Depth`: a full
  stream blocks the writer, matching hardware behavior. The compatibility
  alias `tapa::hls::stream<T, Depth>` is the exception: its simulation depth is
  effectively infinite while its hardware depth remains `Depth`.
- Software simulation copies the elements of a `tapa::stream` byte for byte,
  so `T` must be trivially destructible, as every synthesizable type is: a
  `std::string`, `std::vector` or smart pointer is a compile error. Only
  `tapa::hls::stream` takes any type.
- Stream depth affects FPGA resource usage. Start with the default depth of
  2 and increase it only when a measured burst or rate mismatch requires
  more storage. Check the synthesis resource report after changing it. A
  stream holds at least one element: `tapa analyze` rejects a depth of 0.

---

## Common mistakes

**Wrong** — stream passed by value (loses the required endpoint identity and
is rejected by TAPA):

```cpp
void Leaf(tapa::istream<float> in) { /* ... */ }  // missing &
```

**Right** — stream passed by reference:

```cpp
void Leaf(tapa::istream<float>& in) { /* ... */ }
```

**Wrong** — a channel where a port belongs (a `stream` has no direction, so
there is no interface for it to become):

```cpp
void Task(tapa::stream<float>& data_q) { /* ... */ }
```

```
error: tapa::stream parameter 'data_q' declares a channel rather than a port;
a task reads a channel through tapa::istream and writes one through
tapa::ostream. tapa::stream belongs in the body of an upper task, where it
connects two invocations
```

**Right** — the channel stays in the upper task, the port carries a direction:

```cpp
void Task(tapa::ostream<float>& data_q) { /* ... */ }

void Upper() {
  tapa::stream<float> data_q("data_q");
  tapa::task().invoke(Task, data_q);
}
```

---

## See also

- [Tasks](tasks.md)
- [Deadlocks & Hangs](../troubleshoot/deadlocks-and-hangs.md)
- [C++ API](../reference/api.md)

---

**Next step:** [Memory Access: mmap](mmap.md)
