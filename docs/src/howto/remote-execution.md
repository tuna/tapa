# Remote Execution

**Purpose:** Offload TAPA vendor-tool steps to a remote Linux machine over SSH.

**When to use this:** When your development machine lacks Xilinx/AMD tools
or when you want to delegate long-running synthesis and implementation to a
dedicated Linux build server. The local machine must still run `tapacc`.

## What you need

- SSH access to a Linux machine with Vitis HLS and/or Vivado installed
- `g++` on the remote machine — Vitis HLS includes system C/C++ headers, so a
  minimal remote (for example a fresh container) fails with
  `'features.h' file not found` without it
- The path to `settings64.sh` on the remote machine
- TAPA installed locally, including a working `tapacc` (analysis always runs
  locally)

## How remote execution works

TAPA splits work between local and remote:

| Step | Runs where |
|------|------------|
| `tapa analyze` (runs `tapacc`) | Always local |
| `tapa synth` (Vitis HLS synthesis) | Remote when `--remote-host` is set |
| `tapa floorplan` with `--run-impl` or `--dse` (`v++ --link` or SLASH) | Remote when `--remote-host` is set |
| `tapa pack` for `xilinx-vitis` or `xilinx-slash` (XO / SLASH packaging) | Remote when `--remote-host` is set |
| `tapa pack` for `xilinx-hls` (ZIP assembly) | Always local |
| `tapa link` (`v++` for Vitis, `slashkit`/Vivado for SLASH) | Remote when `--remote-host` is set |
| Remote job inputs and outputs | Transferred automatically by TAPA |

Planning-only `tapa floorplan` (no `--run-impl`, no `--dse`) needs no vendor tool and always runs locally, using the local `cbc` solver.

TAPA stages each remote job's inputs and returns its outputs automatically.
You do not need to copy source trees or compiler artifacts by hand. The
linker programs (`v++`, `slashkit`, `vivado`) are looked up by name on the
remote host — never on your local `PATH` — and `tapa link`'s `--vpp`,
`--slashkit`, and `--vivado` flags rename them there. A SLASH link needs the
V80 static shell installed wherever `slashkit` runs; see
[Alveo V80 with SLASH](v80.md).

```admonish warning
Remote execution applies to the **`tapa` compiler only**. TAPA does not
relocate the host executable built with `tapa g++`: software simulation, fast
cosimulation, hardware emulation, and on-board execution run wherever you
launch that executable. There is no runtime equivalent of `--remote-host`.
For fast cosimulation with xsim, that machine needs Vivado; with Verilator,
it needs Verilator. Hardware emulation and on-board execution need XRT and
the appropriate platform or board. Run the host over SSH yourself if those
resources are on the build server.
```

## Commands

### Inline remote flags

```bash
tapa \
  --work-dir work.out \
  --remote-host alice@build-server.example.com:22 \
  --remote-key-file ~/.ssh/id_ed25519 \
  --remote-xilinx-settings /opt/Xilinx/Vitis/2024.1/settings64.sh \
  compile \
  --top VecAdd \
  --part-num xcu55c-fsvh2892-2L-e \
  --clock-period 3.33 \
  -f vadd.cpp \
  -o vadd.xo
```

### Parallel HLS jobs on the remote host

Use `-j` to run up to N Vitis HLS processes in parallel on the remote machine:

```bash
tapa \
  --work-dir work.out \
  --remote-host alice@build-server.example.com \
  --remote-xilinx-settings /opt/Xilinx/Vitis/2024.1/settings64.sh \
  synth \
  -j 8 \
  ...
```

```admonish note
`TAPA_CONCURRENCY` and `-j` are **different controls**:

- `TAPA_CONCURRENCY` controls the number of parallel software-simulation threads used by the host runtime during functional simulation (`tapa::invoke` with no bitstream). It has no effect on HLS or remote execution.
- `-j` (passed to `tapa synth`) controls how many Vitis HLS processes run in parallel **on the remote host**.

Keep `-j` at or below the number of cores available on the remote machine.
```

### Reusing the SSH connection

To avoid establishing a new TCP connection on every `tapa` invocation, use connection multiplexing with a persistent socket directory:

```bash
tapa \
  --work-dir work.out \
  --remote-host alice@build-server.example.com \
  --remote-ssh-control-dir ~/.ssh/tapa-mux \
  --remote-ssh-control-persist 4h \
  --remote-xilinx-settings /opt/Xilinx/Vitis/2024.1/settings64.sh \
  compile \
  ...
```

The master connection stays alive for 4 hours after the last client closes. Subsequent `tapa` invocations within that window reuse the existing TCP connection.

## Remote flags reference

| Flag | Description |
|------|-------------|
| `--remote-host user@host[:port]` | Remote Linux host for vendor tools. Overrides the configured host and any user/port explicitly included. Omitted fields come from `~/.taparc`, or default to the current username and port 22 without a config. |
| `--remote-key-file PATH` | SSH private key for authentication. If omitted, OpenSSH uses its normal agent, config, and default identity files. |
| `--remote-xilinx-settings PATH` | Path to `settings64.sh` **on the remote host**. TAPA sources this before invoking Vitis HLS. |
| `--remote-ssh-control-dir DIR` | Local directory for OpenSSH multiplex control sockets. Share across invocations to reuse the master connection. |
| `--remote-ssh-control-persist DURATION` | How long the master socket stays alive after the last connection closes (e.g., `30m`, `4h`). Default: `30m`. |
| `--remote-disable-ssh-mux` | Disable SSH connection multiplexing. Each SSH/SCP call opens a fresh connection. Use this when the remote host or a proxy does not support `ControlMaster`. |

## Persistent configuration via `~/.taparc`

Instead of repeating remote flags on every invocation, store them in `~/.taparc`:

```yaml
remote:
  host: build-server.example.com
  user: alice
  port: 22
  key_file: ~/.ssh/id_ed25519
  xilinx_settings: /opt/Xilinx/Vitis/2024.1/settings64.sh
  work_dir: /tmp/tapa-remote
  ssh_control_dir: ~/.ssh/tapa-mux
  ssh_control_persist: 4h
  ssh_multiplex: true
```

CLI flags override the corresponding `~/.taparc` values. `--remote-host`
replaces `host` and also replaces `user` and `port` only when they are present
in the command-line value; omitted fields retain the config values.

## Validation

After a successful compile, the requested artifact is available at the local
`-o` path. A Vitis XO packaged on the remote host is transferred back
automatically. Check it with:

```bash
ls -lh vadd.xo
```

## If something goes wrong

```admonish warning
**SSH connection refused or timeout**: Verify the host, port, and that your key is accepted with `ssh -i ~/.ssh/id_ed25519 alice@build-server.example.com`.

**`settings64.sh` not found**: Confirm the path is correct on the remote machine with `ssh alice@build-server.example.com ls /opt/Xilinx/Vitis/2024.1/settings64.sh`.

**ControlMaster errors**: If the remote host or an intermediary proxy does not support SSH multiplexing, add `--remote-disable-ssh-mux` to your invocation.

**Unexpected user or port**: `--remote-host` replaces only the fields you
specify. For example, `--remote-host alice@build.example` retains `port` from
`~/.taparc`; without a configured port it defaults to 22. Include
`user@host:port` explicitly when you want to override both user and port.
```
