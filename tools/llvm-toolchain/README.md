# Hermetic LLVM toolchain

TAPA's Bazel C/C++ toolchain comes from `toolchains_llvm`, which by default
downloads the **official prebuilt** clang+llvm release archives. Those
binaries link `libxml2.so.2`, which Ubuntu ≥ 24.04 (and other modern
distros) no longer ship — so `ld.lld`, `clang`, and the tblgen host tools
fail inside the sandbox with `libxml2.so.2: cannot open shared object file`.

This directory builds our own archives **from source with every optional
host-library dependency disabled**, packaged with the exact upstream layout
so `toolchains_llvm` can consume them via per-distribution url/sha256
overrides. The only remaining runtime libs are the build distro's own
libc/libm/libstdc++ (binaries are built per-distro, matching the
`llvm_versions` keys in `MODULE.bazel`).

## Kill list

| CMake flag | Removes | Why safe |
|---|---|---|
| `LLVM_ENABLE_LIBXML2=OFF` | `libxml2.so.2` | Only used for Windows manifest/DIA support; dead weight on Linux. **This is the actual breaker.** |
| `LLVM_ENABLE_ZLIB=OFF` | `libz.so.1` | Only enables `-gz` compressed debug sections; our builds don't use them. |
| `LLVM_ENABLE_ZSTD=OFF` | `libzstd` | Same class as zlib (debug compression). |
| `LLVM_ENABLE_TERMINFO=OFF` | `libtinfo`/`libncurses` | Colored terminal diagnostics in llvm tools only. |
| `LLVM_ENABLE_LIBEDIT=OFF` | `libedit` | Line editing for interactive clang-repl etc. |
| `LLVM_ENABLE_LIBPFM=OFF` | `libpfm` | perf monitoring in llvm-exegesis (not shipped). |
| `LLVM_ENABLE_CURL=OFF` / `LLVM_ENABLE_HTTPLIB=OFF` | `libcurl` | debuginfod client support. |

Tests, docs, examples, benchmarks, and OCaml bindings are also excluded.
Targets: `X86;AArch64` (matches `tapa-llvm-project`'s `llvm_targets`).
Runtimes: `compiler-rt` only (required for the `build:asan` config);
stdlib stays the distro `libstdc++` per the existing `llvm.toolchain`
`stdlib` map.

`build-llvm-toolchain.sh` **fails the build** (`verify_hermetic`) if any
shipped binary still references a killed library, and smoke-tests C++17
`<filesystem>` and `-fsanitize=address` with the new toolchain.

## Build one distro locally

```sh
docker run --rm -v "$PWD":/src:ro -v /tmp/llvm-out:/out \
  ubuntu:22.04 bash /src/tools/llvm-toolchain/build-llvm-toolchain.sh \
  18.1.8 ubuntu-22.04 /tmp/llvm-build
# artifact + sha256 land in /tmp/llvm-out
```

All distros: run the `Build LLVM toolchain (hermetic)` workflow
(Actions → workflow_dispatch). It publishes
`clang+llvm-<ver>-x86_64-linux-gnu-<label>.tar.xz` (+ `.sha256`) as assets
of the chosen release tag. RHEL keys build on AlmaLinux (ABI-equivalent,
no subscription needed).

## Wiring the assets into Bazel (after a build publishes them)

`toolchains_llvm`'s bzlmod `llvm.toolchain` tag forwards the repo-rule
attrs, so overrides go directly in `MODULE.bazel` next to `llvm_versions`:

```python
llvm.toolchain(
    llvm_versions = { ... },          # unchanged
    stdlib = { ... },                 # unchanged
    urls = {
        "ubuntu-22.04-x86_64": ["https://github.com/tuna/tapa/releases/download/llvm-toolchain/clang+llvm-18.1.8-x86_64-linux-gnu-ubuntu-22.04.tar.xz"],
        # ... one entry per llvm_versions key ...
        # all rhel-8.x keys point at the single rhel-8 asset (same for 9.x)
    },
    sha256 = {
        "ubuntu-22.04-x86_64": "<from the .sha256 file>",
        # ...
    },
    strip_prefix = {
        # each archive contains a single top dir named like itself
        "ubuntu-22.04-x86_64": "clang+llvm-18.1.8-x86_64-linux-gnu-ubuntu-22.04",
        # ...
    },
)
```

Land that change only with real published URLs + sha256s; until then the
default upstream archives remain the source (with the host-lib caveat).

## Version policy

Ubuntu keys stay on 18.1.8; RHEL keys stay on 14.0.0 (matching the current
`llvm_versions` pins — uplifting RHEL to 18.x is a separate decision).
Pinned source sha256s live in `build-llvm-toolchain.sh` (`SRC_SHA256`);
add a row when bumping.
