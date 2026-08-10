#!/bin/sh
# Point apt at the TUNA mirrors: the CI runner sits next to TUNA. Plain http
# is deliberate: the base images have no ca-certificates yet at this point,
# and apt authenticates the archive with its own GPG signatures regardless
# of the transport. Handles both the classic sources.list and the deb822
# .sources layout that noble uses. Order matters: debian-security before
# debian, so the longer path is not eaten by the shorter one.
set -eu
for f in /etc/apt/sources.list /etc/apt/sources.list.d/*.list /etc/apt/sources.list.d/*.sources; do
  [ -f "$f" ] || continue
  sed -i \
    -e 's|https\?://deb\.debian\.org/debian-security|http://mirrors.tuna.tsinghua.edu.cn/debian-security|g' \
    -e 's|https\?://security\.debian\.org/debian-security|http://mirrors.tuna.tsinghua.edu.cn/debian-security|g' \
    -e 's|https\?://security\.debian\.org|http://mirrors.tuna.tsinghua.edu.cn/debian-security|g' \
    -e 's|https\?://deb\.debian\.org/debian|http://mirrors.tuna.tsinghua.edu.cn/debian|g' \
    -e 's|https\?://archive\.ubuntu\.com/ubuntu|http://mirrors.tuna.tsinghua.edu.cn/ubuntu|g' \
    -e 's|https\?://security\.ubuntu\.com/ubuntu|http://mirrors.tuna.tsinghua.edu.cn/ubuntu|g' \
    "$f"
done
