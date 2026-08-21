# Provenance: `ignore-shell` golden case

## Design: hand-authored, modeled on the `async-mmap` case

This case pins the `SynthTarget::Ignore` path through
`generate_rtl`'s `ArtifactManifest` — the `template_files` output that
previously had only unit-test coverage (`shell.v` in
`generate_rtl_tests.rs`), never a golden pin.

`design.json` follows the exact schema of the `async-mmap` case (same
hand-adapted style; a real `tapacc` fixture is out of budget for the same
reason documented there): top task `ShellTop` (upper, `hls`) with a scalar
`n` and an external `ostream out`; one child `CustomShell` (lower,
`ignore`) whose scalar binds the top's `n` and whose `ostream` binds the
top's external stream. The ports make the rendered template shell
non-trivial — it exercises the scalar and stream port spellings of
`template::render_task_template`.

## HLS inputs

`inputs/` contains fixtures only for non-`Ignore` tasks — just
`ShellTop.v`, a hand-authored, interface-faithful minimized Vitis-HLS
fixture in the same style as the `async-mmap` top, minus the m_axi ports
(`ShellTop` has no mmap port; `n` arrives through `s_axi_control`). The
`Ignore` task has no HLS input by construction: codegen builds its
authoritative port-only shell from the topology.

## Blessed output

Same rules as the other cases (`rtl/` = the manifest's design-specific
slice, support assets under `_assets/`, normalized). This case exercises:
the `ignore-task-shells` pass (authoritative shell into `module_map`), the
template emission in `CollectOutputs` — blessed here as
`template/CustomShell.v` plus its `rtl/CustomShell.v` package placeholder —
the children wiring of an ignored instance (scalar propagation from
`s_axi_control`, external-stream passthrough), and the monolithic-FSM
control path for the top.

## Regeneration review (2026-08-21)

One line: `ShellTop.v` gains `.out_peek('d0)` on the shell instance,
from `5220c70f` tying off unused HLS peek inputs — previously the port
was left unconnected. `design.json` carries the shared representation
changes only.
