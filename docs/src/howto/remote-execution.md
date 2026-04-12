# Remote Execution

**Purpose:** Offload TAPA vendor-tool steps to a remote Linux machine over SSH.

**When to use this:** When your development machine is macOS (where Xilinx/AMD tools are unavailable) or when you want to delegate long-running HLS synthesis and implementation steps to a dedicated Linux build server.

## What you need

- SSH access to a Linux machine with Vitis HLS and/or Vivado installed
- The path to `settings64.sh` on the remote machine
- TAPA installed locally (the `tapa analyze` step always runs locally)

## How remote execution works

TAPA splits work between local and remote:

| Step | Runs where |
|------|------------|
| `tapa analyze` (runs `tapa-cpp` and `tapacc`) | Always local |
| `tapa synth` (Vitis HLS synthesis) | Remote when `--remote-host` is set |
| `tapa pack` (IP packaging) | Remote when `--remote-host` is set |
| Host fast-cosim runtime (`--bitstream=*.xo`) | Remote when `--remote-host` is set |
| File transfer (`.xo`, `.zip` artifacts) | Handled automatically by TAPA |

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
  --part-num xcu280-fsvh2892-2L-e \
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
| `--remote-host user@host[:port]` | Remote Linux host for vendor tools. Omit user to use the current local username; omit port to use 22. |
| `--remote-key-file PATH` | SSH private key for authentication. Defaults to the SSH agent or `~/.ssh/id_rsa`. |
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

CLI flags always override the corresponding `~/.taparc` values. In particular, `--remote-host` replaces the `host`, `user`, and `port` fields from the config file.

## Validation

After a successful remote compile, the `.xo` artifact is automatically transferred back to your local machine. Check for it:

```bash
ls -lh vadd.xo
```

TAPA prints transfer progress and the remote Vitis HLS log to standard output during the run.

## If something goes wrong

```admonish warning
**SSH connection refused or timeout**: Verify the host, port, and that your key is accepted with `ssh -i ~/.ssh/id_ed25519 alice@build-server.example.com`.

**`settings64.sh` not found**: Confirm the path is correct on the remote machine with `ssh alice@build-server.example.com ls /opt/Xilinx/Vitis/2024.1/settings64.sh`.

**ControlMaster errors**: If the remote host or an intermediary proxy does not support SSH multiplexing, add `--remote-disable-ssh-mux` to your invocation.

**Port conflicts with `~/.taparc`**: If you omit the port in `--remote-host`, TAPA defaults to port 22 — it does **not** fall back to the `port` field from `~/.taparc`. Always include the port explicitly (e.g., `user@host:2222`) when the remote host listens on a non-standard port.
```

---

**Next step:** [Using the Visualizer](visualizer.md)
