# Your First Run

Run your first TAPA software simulation without FPGA hardware.

## When to use this

Use this guide when you are learning TAPA for the first time, or when you
want to quickly verify a design's correctness without synthesizing RTL or
running on physical hardware.

## What you need

- TAPA installed — see [Installation](installation.md)
- `g++` 7.5.0 or newer (check with `g++ --version`)
- The vadd example files: [`vadd.cpp`](https://github.com/tuna/tapa/blob/main/tests/apps/vadd/vadd.cpp) and [`vadd-host.cpp`](https://github.com/tuna/tapa/blob/main/tests/apps/vadd/vadd-host.cpp)

## Commands

Compile the kernel and host code together using the `tapa g++` wrapper, then
run the resulting binary with no arguments to trigger software simulation:

```bash
tapa g++ -- vadd.cpp vadd-host.cpp -o vadd
./vadd
```

```admonish note
`tapa g++` is a wrapper around the GNU C++ compiler that automatically
includes the necessary TAPA headers and libraries. It prints the underlying
`g++` command it invokes for reference.

Both the kernel file (`vadd.cpp`) and the host file (`vadd-host.cpp`) must
be passed in the same command. The kernel file is used for software
simulation.
```

## Expected output

```
I20000101 00:00:00.000000 0000000 task.h:66] running software simulation with TAPA library
kernel time: 1.19429 s
PASS!
```

## What this proves

The `PASS!` line confirms the vector addition produced correct results. The
first line shows that TAPA executed the kernel on the CPU using its
coroutine-based software simulator — no FPGA or Xilinx tools were involved.

## If something goes wrong

If the build fails, the binary hangs, or the output shows `FAIL!`, see
[Your First Debug Cycle](first-debug.md).

## Next step

[The Programming Model](../concepts/programming-model.md)
