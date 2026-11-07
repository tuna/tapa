# Provenance: `frame-views` golden case

## Design

`design.json` is the post-synthesis graph (`tapa.json`'s `graph`) of
`tests/functional/frame-views/corner.cpp` as `tapa compile` produced it with
tapacc and Vitis HLS 2025.2 (`--part-num xcu250-figd2104-2L-e`, the Bazel
`tapa_xo` default): top `CornerTurn` with mmaps `src` and `dst`; `Load`
writes the element stream `rows` into `Turn`'s frame port; `Turn` writes
frames of 1024 to `TurnOut` through `once`; `TurnOut` reads them and writes
the element stream `twice` (an ordinary FIFO); the upper task `Drain` declares
`cols`, into which `TurnIn` writes frames from it and from which `Store`
reads elements. The frame channels are two banks deep (depth 2048).

## HLS inputs

`inputs/*.v` keep the module header and every port and parameter
declaration of the HLS output for each task, byte for byte, with the
bodies replaced by zero-tied outputs (codegen reads only interfaces here;
the zero-driven pruning it applies to `async_mmap` bridges does not touch
these tasks). `Turn.v` shows a frame port's interface: an `ap_memory` port
(`*_buf_address0` of 18 bits for 1024 elements in up to 256 banks, `*_ce0`,
`*_q0` or `*_we0`/`*_d0`) and the `*_acquire_*` / `*_release_*` token FIFOs;
`TurnOut.v` and `TurnIn.v` show one beside an ordinary stream port.

## Blessed output

Same rules as the other cases. This case exercises the `frame_channel`
instances for every pairing of ends: an element stream into a frame port
(`rows`), two frame ports (`once`), and a frame port into an element
stream (`cols`, inside the upper task `Drain`), with their bank count,
widths and wiring, next to a FIFO (`twice`) between two tasks that also
have frame ports.
