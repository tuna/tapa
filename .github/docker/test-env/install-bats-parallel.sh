#!/bin/bash
# Install bats (from source, GitHub clone) and GNU parallel (from source).
# Expects git, curl, a C toolchain and PARALLEL_VERSION/PARALLEL_SHA256 in
# the environment; runs under bash (the RHEL-family images have it).
set -eu
cd /tmp
# Retry: GitHub is the one host in this build with no usable mirror.
for attempt in 1 2 3 4 5; do
  git clone --depth 1 --branch "${BATS_VERSION}" \
    https://github.com/bats-core/bats-core.git bats-core && break
  echo "bats clone attempt ${attempt} failed; retrying" >&2
  rm -rf bats-core
  sleep $((attempt * 10))
done
cd bats-core
./install.sh /usr/local
cd /tmp
# TUNA first, then the GNU mirror redirector. `-f` keeps an HTTP error
# body from being piped into tar, and the checksum catches truncation.
for url in \
  "https://mirrors.tuna.tsinghua.edu.cn/gnu/parallel/parallel-${PARALLEL_VERSION}.tar.bz2" \
  "https://ftpmirror.gnu.org/gnu/parallel/parallel-${PARALLEL_VERSION}.tar.bz2"
do
  curl -fsSL --connect-timeout 20 --max-time 900 \
       --retry 5 --retry-delay 5 --retry-all-errors \
       -o parallel.tar.bz2 "${url}" && break
  echo "parallel download failed: ${url}" >&2
done
echo "${PARALLEL_SHA256}  parallel.tar.bz2" | sha256sum -c -
tar -xjf parallel.tar.bz2
cd "parallel-${PARALLEL_VERSION}"
./configure
make install -j "$(nproc)"
