#!/usr/bin/env bash
# Build a hermetic LLVM/Clang toolchain from source and package it with the
# exact layout of the upstream clang+llvm release archives so it can be
# consumed by toolchains_llvm via per-distribution url/sha256 overrides.
#
# "Hermetic" here: the shipped binaries link only libc/libm/libstdc++ of the
# build distro itself. Every optional host-library dependency LLVM can pick
# up is disabled (see KILL LIST below) — most importantly libxml2.so.2, which
# the official prebuilt binaries require and Ubuntu >= 24.04 no longer ships.
#
# Usage (normally inside docker, see README.md):
#   build-llvm-toolchain.sh <llvm-version> <distro-label> [workdir]
# Example:
#   build-llvm-toolchain.sh 18.1.8 ubuntu-22.04 /tmp/llvm-build
set -euo pipefail

LLVM_VERSION="${1:?llvm version, e.g. 18.1.8}"
DISTRO="${2:?distro label matching toolchains_llvm keys, e.g. ubuntu-22.04}"
WORKDIR="${3:-/tmp/llvm-build}"
JOBS="${JOBS:-$(nproc)}"
LINK_JOBS="${LINK_JOBS:-2}"  # ld steps are RAM-hungry; keep low on small boxes
OUT_DIR="${OUT_DIR:-/out}"

NAME="clang+llvm-${LLVM_VERSION}-x86_64-linux-gnu-${DISTRO}"
SRC_DIR="${WORKDIR}/llvm-project"
BUILD_DIR="${WORKDIR}/build"
PREFIX="${WORKDIR}/install"

# Pinned source archives. Add a row when bumping versions.
declare -A SRC_SHA256=(
  ["18.1.8"]="0b58557a6d32ceee97c8d533a59b9212d87e0fc4d2833924eb6c611247db2f2a"
  ["14.0.0"]="35ce9edbc8f774fe07c8f4acdf89ec8ac695c8016c165dd86b8d10e7cba07e23"
)

log() { echo "[llvm-build] $*"; }

install_build_deps() {
  if command -v apt-get > /dev/null; then
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends \
      ca-certificates curl xz-utils cmake ninja-build python3 make binutils
    # LLVM >= 16 needs a C++17 toolchain with a working <filesystem>; the
    # distro default gcc on ubuntu <= 20.04 is too old, pull a newer one.
    if ! echo 'int main(){}' | g++ -std=c++17 -x c++ - -o /dev/null 2>/dev/null; then
      apt-get install -y -qq software-properties-common
      add-apt-repository -y ppa:ubuntu-toolchain-r/test
      apt-get update -qq
      apt-get install -y -qq gcc-11 g++-11
      export CC=gcc-11 CXX=g++-11
    fi
  elif command -v dnf > /dev/null; then
    dnf install -y -q ca-certificates curl xz cmake ninja-build python3 make binutils
    if ! echo 'int main(){}' | g++ -std=c++17 -x c++ - -o /dev/null 2>/dev/null; then
      dnf install -y -q gcc-toolset-12-gcc gcc-toolset-12-gcc-c++ \
        || dnf install -y -q gcc-toolset-11-gcc gcc-toolset-11-gcc-c++
      # shellcheck disable=SC1091
      source /opt/rh/gcc-toolset-*/enable
    fi
  else
    log "ERROR: no supported package manager (apt/dnf) in this image"; exit 1
  fi
  log "compiler: $(${CXX:-g++} --version | head -1)"
}

fetch_source() {
  local url="https://github.com/llvm/llvm-project/releases/download/llvmorg-${LLVM_VERSION}/llvm-project-${LLVM_VERSION}.src.tar.xz"
  local sha="${SRC_SHA256[$LLVM_VERSION]:?no pinned sha256 for $LLVM_VERSION}"
  mkdir -p "$WORKDIR"
  log "fetching $url"
  curl -fSL --retry 3 -o "${WORKDIR}/src.tar.xz" "$url"
  echo "${sha}  ${WORKDIR}/src.tar.xz" | sha256sum -c -
  mkdir -p "$SRC_DIR"
  tar -xJf "${WORKDIR}/src.tar.xz" -C "$SRC_DIR" --strip-components=1
}

configure() {
  cmake -S "$SRC_DIR/llvm" -B "$BUILD_DIR" -G Ninja \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX="$PREFIX" \
    -DLLVM_ENABLE_PROJECTS="clang;lld" \
    -DLLVM_ENABLE_RUNTIMES="compiler-rt" \
    -DLLVM_TARGETS_TO_BUILD="X86;AArch64" \
    `# ── KILL LIST: optional host-library dependencies, all OFF ──` \
    -DLLVM_ENABLE_LIBXML2=OFF \
    -DLLVM_ENABLE_ZLIB=OFF \
    -DLLVM_ENABLE_ZSTD=OFF \
    -DLLVM_ENABLE_TERMINFO=OFF \
    -DLLVM_ENABLE_LIBEDIT=OFF \
    -DLLVM_ENABLE_LIBPFM=OFF \
    -DLLVM_ENABLE_CURL=OFF \
    -DLLVM_ENABLE_HTTPLIB=OFF \
    `# ── Distribution slimming ──` \
    -DLLVM_INCLUDE_TESTS=OFF \
    -DLLVM_INCLUDE_BENCHMARKS=OFF \
    -DLLVM_INCLUDE_EXAMPLES=OFF \
    -DLLVM_BUILD_DOCS=OFF \
    -DLLVM_ENABLE_DOXYGEN=OFF \
    -DLLVM_ENABLE_SPHINX=OFF \
    -DLLVM_ENABLE_OCAMLDOC=OFF \
    -DLLVM_ENABLE_BINDINGS=OFF \
    -DLLVM_ENABLE_ASSERTIONS=OFF \
    -DLLVM_OPTIMIZED_TABLEGEN=ON \
    -DCLANG_VENDOR="TAPA" \
    -DLLVM_PARALLEL_LINK_JOBS="$LINK_JOBS"
}

build_and_package() {
  cmake --build "$BUILD_DIR" -j "$JOBS"
  cmake --install "$BUILD_DIR" > /dev/null
  # The build tree peaks at tens of GB; drop it before packaging (CI disks).
  rm -rf "$BUILD_DIR"

  # Layout identical to upstream archives: one top dir named like the archive.
  mkdir -p "${WORKDIR}/pkg/${NAME}"
  cp -a "${PREFIX}/." "${WORKDIR}/pkg/${NAME}/"
  mkdir -p "$OUT_DIR"
  (cd "${WORKDIR}/pkg" && tar -cJf "${OUT_DIR}/${NAME}.tar.xz" "$NAME")
  (cd "$OUT_DIR" && sha256sum "${NAME}.tar.xz" > "${NAME}.tar.xz.sha256")
  log "packaged ${OUT_DIR}/${NAME}.tar.xz"
  cat "${OUT_DIR}/${NAME}.tar.xz.sha256"
}

verify_hermetic() {
  # Fail loudly if any disabled host library snuck back in.
  local bad='libxml2|libz\.so|libzstd|libtinfo|libncurses|libedit|libcurl|libpfm'
  local hits=""
  for f in "${WORKDIR}/pkg/${NAME}"/bin/*; do
    [ -f "$f" ] || continue
    hits+=$(ldd "$f" 2>/dev/null | grep -E "$bad" || true)
  done
  for f in "${WORKDIR}/pkg/${NAME}"/lib/*.so*; do
    [ -f "$f" ] || continue
    hits+=$(ldd "$f" 2>/dev/null | grep -E "$bad" || true)
  done
  if [ -n "$hits" ]; then
    log "ERROR: forbidden host libraries referenced:"; echo "$hits"; exit 1
  fi
  log "hermeticity check passed (no libxml2/zlib/zstd/tinfo/edit/curl/pfm)"

  # Smoke: compile and run a C++17 + ASan binary with the new toolchain.
  local smoke="${WORKDIR}/smoke"
  mkdir -p "$smoke"
  printf '#include <filesystem>\n#include <iostream>\nint main(){std::cout<<std::filesystem::path{"ok"}.string()<<"\\n";int*p=new int(3);delete p;}\n' > "$smoke/t.cpp"
  "${WORKDIR}/pkg/${NAME}/bin/clang++" -std=c++17 "$smoke/t.cpp" -o "$smoke/t"
  [ "$("$smoke/t")" = "ok" ]
  "${WORKDIR}/pkg/${NAME}/bin/clang++" -std=c++17 -fsanitize=address "$smoke/t.cpp" -o "$smoke/t_asan"
  [ "$("$smoke/t_asan")" = "ok" ]
  log "smoke tests passed (c++17 filesystem, asan)"
}

install_build_deps
fetch_source
configure
build_and_package
verify_hermetic
log "done"
