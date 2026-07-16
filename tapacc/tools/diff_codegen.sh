#!/bin/bash
# Diff the per-task generated code of the OLD tapacc (via `tapa analyze`) against
# the rewritten tool (`//tapacc:tapacc`) on the SAME flattened source, for one
# kernel. This is the code-emission half of the equivalence gate for the tapacc
# rewrite; the graph-metadata half waits on the tapa-ir schema.
#
#   usage: diff_codegen.sh <name> <src.cpp> <top> [target]
#   env:   XINC=<dir>   extra -isystem dir (e.g. Vivado HLS headers for ap_int.h)
#
# Requires: a built //tapacc:tapacc (bazel builds it on demand), macOS `xcrun`
# for the sysroot, and clang builtin headers resolved via tapacc's runfiles.
set -uo pipefail

NAME="$1"; CPP="$2"; TOP="$3"; TGT="${4:-xilinx-hls}"
XINC="${XINC:-}"
ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
WORK="$(mktemp -d)/${NAME}"; mkdir -p "$WORK"
SDK="$(xcrun --show-sdk-path 2>/dev/null || echo /)"
cd "$ROOT" || exit 9

bazel build //tapacc:tapacc >/dev/null 2>&1 || { echo "$NAME: BUILD FAILED"; exit 3; }
RES="$(cd "$ROOT"/bazel-bin/tapacc/tapacc.runfiles/+http_archive+tapa-llvm-project/clang/staging && pwd)"
XC=(); [ -n "$XINC" ] && XC=(-c "-isystem$XINC")
XI=(); [ -n "$XINC" ] && XI=(-isystem "$XINC")

# OLD: real tapa analyze -> graph.json (old per-task code) + flattened source.
bazel run -- //tapa-core:tapa --work-dir "$WORK" analyze -f "$CPP" -t "$TOP" --target "$TGT" \
  -c "-isystem$ROOT/tapa-lib" -c "-isystem$ROOT/fpga-runtime" ${XC[@]+"${XC[@]}"} >"$WORK/analyze.log" 2>&1
[ -f "$WORK/graph.json" ] || { echo "$NAME: OLD ANALYZE FAILED"; grep -iE 'error:|fatal' "$WORK/analyze.log" | head -2; exit 1; }
FLAT="$(ls "$WORK"/flatten/flatten-*.cpp 2>/dev/null | head -1)"

# NEW: tapacc on the same flattened source -> per-task code JSON.
bazel run -- //tapacc:tapacc "$FLAT" -top "$TOP" --target "$TGT" -- \
  -resource-dir "$RES" -std=c++14 -isystem "$ROOT/tapa-lib" -isystem "$ROOT/fpga-runtime" ${XI[@]+"${XI[@]}"} \
  -isysroot "$SDK" -Wno-attributes -Wno-unknown-pragmas -Wno-unused-label \
  -DTAPA_TARGET_DEVICE_ -DTAPA_TARGET_STUB_ >"$WORK/new.json" 2>"$WORK/new.err"
python3 -c "import json;json.load(open('$WORK/new.json'))" 2>/dev/null || { echo "$NAME: NEW tapacc FAILED"; tail -2 "$WORK/new.err"; exit 2; }

# Compare per-task code, whitespace-normalized (the tools differ only in layout).
python3 - "$NAME" "$WORK/graph.json" "$WORK/new.json" <<'PY'
import json, sys, re, difflib
name, old = sys.argv[1], json.load(open(sys.argv[2]))['tasks']
new = json.load(open(sys.argv[3]))['tasks']
def norm(c): return [l for l in (re.sub(r'[ \t]+',' ',x).rstrip() for x in c.splitlines()) if l.strip()]
ident = diff = 0; details = []
for t in sorted(set(old) | set(new)):
    if t not in old or t not in new:
        diff += 1; details.append(f"    MISSING {t} (old={'y' if t in old else 'n'} new={'y' if t in new else 'n'})"); continue
    o, n = norm(old[t]['code']), norm(new[t]['code'])
    if o == n:
        ident += 1
    else:
        diff += 1
        dl = [l for l in difflib.unified_diff(o, n, lineterm='') if l[:1] in '+-' and l[:3] not in ('+++','---')]
        details.append(f"    DIFF {t}: -{sum(1 for l in dl if l[0]=='-')}/+{sum(1 for l in dl if l[0]=='+')}")
print(f"[{'OK' if diff==0 else 'XX'}] {name}: {ident} identical, {diff} differ")
for d in details[:8]: print(d)
sys.exit(0 if diff == 0 else 1)
PY
