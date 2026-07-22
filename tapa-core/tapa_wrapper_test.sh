#!/usr/bin/env bash
set -euo pipefail

test_root="${TEST_TMPDIR}/direct-launch"
launcher="${test_root}/tapa"
runfiles_dir="${launcher}.runfiles"
manifest="${launcher}.runfiles_manifest"
logical_cli="${runfiles_dir}/_main/tapa-core/cargo/bin/tapa"
physical_cli="${test_root}/execroot/tapa-core/cargo/bin/tapa"

wrapper_src="${TEST_SRCDIR}/${TEST_WORKSPACE}/tapa-core/tapa_wrapper.sh"
runfiles_lib="${TEST_SRCDIR}/bazel_tools/tools/bash/runfiles/runfiles.bash"

mkdir -p \
  "$(dirname "${logical_cli}")" \
  "$(dirname "${physical_cli}")" \
  "${runfiles_dir}/bazel_tools/tools/bash/runfiles"
cp "${wrapper_src}" "${launcher}"
ln -s "${physical_cli}" "${logical_cli}"
ln -s "${runfiles_lib}" \
  "${runfiles_dir}/bazel_tools/tools/bash/runfiles/runfiles.bash"

printf '%s\n' \
  '#!/usr/bin/env bash' \
  'printf '\''%s\n'\'' "${TAPA_CLI_SEARCH_ANCHOR:-}"' \
  >"${physical_cli}"
chmod +x "${launcher}" "${physical_cli}"

printf '%s %s\n' \
  'bazel_tools/tools/bash/runfiles/runfiles.bash' "${runfiles_lib}" \
  '_main/tapa-core/cargo/bin/tapa' "${physical_cli}" \
  >"${manifest}"

actual="$(env \
  -u JAVA_RUNFILES \
  -u RUNFILES_DIR \
  -u RUNFILES_MANIFEST_FILE \
  -u RUNFILES_REPO_MAPPING \
  "${launcher}")"

if [[ "${actual}" != "${logical_cli}" ]]; then
  echo "expected logical runfiles anchor: ${logical_cli}" >&2
  echo "actual anchor: ${actual}" >&2
  exit 1
fi
