"""Shared shell helpers for the repo's custom rules."""

def sh_quote(value):
    """Quote a string for POSIX shell single-quoted contexts."""
    return "'" + value.replace("'", "'\"'\"'") + "'"

# The resolve_runfile bash function.
#
# Finds a runfile across the RUNFILES_DIR / $0.runfiles /
# RUNFILES_MANIFEST_FILE layouts. Embed as a `.format()` value (the
# substituted value is inserted verbatim, so no brace escaping).
RESOLVER = """resolve_runfile() {
  local path="$1"
  local workspace="${TEST_WORKSPACE:-_main}"
  local candidate
  for candidate in \\
    "${RUNFILES_DIR:-}/${workspace}/${path}" \\
    "${RUNFILES_DIR:-}/_main/${path}" \\
    "${RUNFILES_DIR:-}/${path}" \\
    "$0.runfiles/${workspace}/${path}" \\
    "$0.runfiles/_main/${path}" \\
    "$0.runfiles/${path}"; do
    if [[ -f "$candidate" ]]; then
      printf '%s\\n' "$candidate"
      return 0
    fi
  done
  if [[ -n "${RUNFILES_MANIFEST_FILE:-}" ]]; then
    for candidate in "${workspace}/${path}" "_main/${path}" "${path}"; do
      local resolved
      resolved="$(grep -m1 "^${candidate} " "${RUNFILES_MANIFEST_FILE}" | cut -d' ' -f2- || true)"
      if [[ -n "$resolved" && -f "$resolved" ]]; then
        printf '%s\\n' "$resolved"
        return 0
      fi
    done
  fi
  echo "missing runfile: $path" >&2
  return 1
}
"""

def runfiles_resolver():
    """The resolve_runfile bash function body."""
    return RESOLVER
