#!/bin/bash

# Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
# All rights reserved. The contributor(s) of this file has/have agreed to the
# RapidStream Contributor License Agreement.

set -e

# Replaces every occurrence of the old (current) version string with the new
# one in each given file, so that VERSION stays the single source of truth for
# the referenced stable version. The old version is matched literally (as a
# fixed string, with dots escaped in the sed pattern), making the replacement
# idempotent and exact. Fails *before* modifying any file if the old version
# string is absent from any of the files, so a version bump can never leave
# stale version references behind silently.
function update_version_references() {
  # NB: distinct names from main()'s readonly variables; bash locals are
  # dynamically scoped and cannot shadow a readonly variable of the same name.
  readonly local old_ver="$1"
  readonly local new_ver="$2"
  shift 2

  local file
  for file in "$@"; do
    if ! grep --quiet --fixed-strings -- "${old_ver}" "${file}"; then
      echo >&2 "Version string ${old_ver} not found in ${file}"
      return 1
    fi
  done
  for file in "$@"; do
    sed --in-place "s/${old_ver//./\\.}/${new_ver}/g" "${file}"
  done
}

# Creates a new commit with the patch version being the current date, if there
# are changes since the last version change.
function main() {
  readonly repo="$(realpath "${0%/*}"/../..)"
  cd "${repo}"

  readonly old_version="$(cat VERSION)"
  if [[ "${old_version}" != *.*.???????? ]]; then
    echo >&2 "Unexpected version string: ${old_version}"
    return 1
  fi

  readonly old_commit="$(git log --format=%H -1 -- VERSION)"
  if git diff --quiet "${old_commit}" -- . :^docs/ :^tests/ :^README.md; then
    echo >&2 "No change since commit ${old_commit}"
    return 0
  fi

  readonly old_version_patch="${old_version##*.}"
  readonly new_version_patch="$(date +%Y%m%d)"
  if ((new_version_patch <= old_version_patch)); then
    echo >&2 "Unexpected date: ${new_version_patch} <= ${old_version_patch}"
    return 1
  fi

  readonly old_version_major_minor="${old_version%${old_version_patch}}"
  readonly new_version="${old_version_major_minor}${new_version_patch}"
  echo "${new_version}" >VERSION

  # Propagate the new version to all files that reference the stable version
  # string, keeping VERSION the single source of truth.
  update_version_references "${old_version}" "${new_version}" \
    install.sh \
    README.md \
    docs/src/start/installation.md

  git add -- VERSION install.sh README.md docs/src/start/installation.md
  git commit --no-verify --message "build: bump version to ${new_version}"
}

main "$@"
