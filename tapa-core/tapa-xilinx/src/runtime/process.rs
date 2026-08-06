//! Subprocess abstraction: the `ToolRunner` trait plus the local
//! `std::process::Command`-backed implementation and a test-only mock.
//!
//! Every tool wrapper speaks through this trait so unit tests never
//! need `vitis_hls` or `vivado` on the host.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use camino::Utf8PathBuf;

use crate::error::{Result, XilinxError};

#[derive(Debug, Clone, Default)]
pub struct ToolInvocation {
    pub program: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub stdin: Option<Vec<u8>>,
    pub cwd: Option<Utf8PathBuf>,
    pub uploads: Vec<Utf8PathBuf>,
    pub downloads: Vec<Utf8PathBuf>,
    pub timeout: Option<Duration>,
}

impl ToolInvocation {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    #[must_use]
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub trait ToolRunner: Send + Sync {
    fn run(&self, inv: &ToolInvocation) -> Result<ToolOutput>;
}

/// Local subprocess runner: inherits the parent process's environment.
/// For Xilinx tools, an existing `settings64.sh` under the applicable
/// `XILINX_*` root is sourced before execution; otherwise bare program names
/// resolve through the caller's `PATH`. `ToolInvocation::env` entries overlay
/// the inherited environment.
///
/// Remote env allowlisting lives in `RemoteToolRunner` where the env
/// crosses a machine boundary. On a single host, the child sees the
/// parent shell's state. When
/// `ToolInvocation::timeout` is set, the child is killed on expiry
/// and the call returns `XilinxError::ToolTimeout`.
#[derive(Debug, Default)]
pub struct LocalToolRunner;

impl LocalToolRunner {
    pub const fn new() -> Self {
        Self
    }
}

/// Tool name → env vars that may hold its installation root, in
/// precedence order. Adding a Xilinx tool is a one-line table edit.
const XILINX_TOOL_ENVS: &[(&str, &[&str])] = &[
    ("vitis_hls", &["XILINX_HLS", "XILINX_VITIS"]),
    ("vivado", &["XILINX_VIVADO", "XILINX_VITIS"]),
    ("v++", &["XILINX_VITIS"]),
];

/// The tool a program string names, normalized: an invocation may spell it
/// as a bare name, an absolute path, or with a `.exe` suffix, and every
/// decision keyed on the tool must agree about which of those it is.
pub(crate) fn xilinx_tool_name(program: &str) -> &str {
    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program)
        .trim_end_matches(".exe")
}

fn xilinx_settings_envs(program: &str) -> &'static [&'static str] {
    let tool = xilinx_tool_name(program);
    XILINX_TOOL_ENVS
        .iter()
        .find_map(|(name, envs)| (*name == tool).then_some(*envs))
        .unwrap_or(&[])
}

fn invocation_env_path(inv: &ToolInvocation, name: &str) -> Option<PathBuf> {
    if let Some(value) = inv.env.get(name) {
        return (!value.trim().is_empty()).then(|| PathBuf::from(value));
    }
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn local_xilinx_settings(inv: &ToolInvocation) -> Option<PathBuf> {
    xilinx_settings_envs(&inv.program).iter().find_map(|name| {
        let root = invocation_env_path(inv, name)?;
        if root.is_file() && root.extension().is_some_and(|ext| ext == "sh") {
            return Some(root);
        }
        let settings = root.join("settings64.sh");
        settings.is_file().then_some(settings)
    })
}

/// Vitis 2025.1+ removed the classic `vitis_hls` executable; the unified
/// `vitis-run` CLI evaluates the same classic HLS Tcl via
/// `--mode hls --tcl <file>`. Only rewrite when the resolved tool root
/// lacks `bin/vitis_hls` but ships `bin/vitis-run`, so every install
/// through 2024.2 keeps its tested entry point.
fn hls_needs_unified_rewrite(tool_root: &Path) -> bool {
    !tool_root.join("bin").join("vitis_hls").exists()
        && tool_root.join("bin").join("vitis-run").exists()
}

/// Translate classic `vitis_hls` argv to `vitis-run` argv: `-f <tcl>`
/// becomes `--tcl <tcl>` (in `vitis-run`, `-f` means `--platform`), and
/// `--mode hls` is always prepended. Other args pass through untouched.
pub(crate) fn unified_hls_args(args: &[String]) -> Vec<String> {
    let mut out = vec!["--mode".to_owned(), "hls".to_owned()];
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "-f" {
            if let Some(tcl) = iter.next() {
                out.push("--tcl".to_owned());
                out.push(tcl.clone());
            } else {
                // A dangling `-f` would vanish here and launch with no
                // script at all; pass it through so the tool complains
                // about the missing argument like classic vitis_hls does.
                out.push(arg.clone());
            }
        } else {
            out.push(arg.clone());
        }
    }
    out
}

fn local_command(inv: &ToolInvocation) -> std::process::Command {
    let Some(settings) = local_xilinx_settings(inv) else {
        let mut cmd = std::process::Command::new(&inv.program);
        cmd.args(&inv.args);
        return cmd;
    };

    // Normalized the same way settings resolution normalizes it: an
    // absolute path or a `.exe` suffix still resolves settings64.sh and
    // takes this branch, so an exact-string test would leave the classic
    // `vitis_hls` name in place on a 2025.1+ install, where it does not
    // exist, and the step would fail with "command not found".
    let unified = xilinx_tool_name(&inv.program) == "vitis_hls"
        && settings.parent().is_some_and(hls_needs_unified_rewrite);
    let (program, args) = if unified {
        ("vitis-run".to_owned(), unified_hls_args(&inv.args))
    } else {
        (inv.program.clone(), inv.args.clone())
    };

    // Keep the settings path and tool argv out of the shell program
    // text. Positional parameters preserve spaces and shell
    // metacharacters exactly while settings64.sh populates PATH and
    // the XILINX_* environment before exec.
    let mut cmd = std::process::Command::new("bash");
    cmd.arg("-c")
        .arg("source \"$1\" && shift && exec \"$@\"")
        .arg("tapa-xilinx-settings")
        .arg(settings)
        .arg(program)
        .args(args);
    cmd
}

fn wait_with_deadline(
    child: &mut std::process::Child,
    deadline: std::time::Instant,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    use wait_timeout::ChildExt as _;
    child.wait_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
}

impl ToolRunner for LocalToolRunner {
    fn run(&self, inv: &ToolInvocation) -> Result<ToolOutput> {
        use std::io::{Read, Write};
        use std::process::Stdio;

        let mut cmd = local_command(inv);
        // Inherit the parent's full env, then overlay `inv.env` so
        // per-invocation entries win.
        for (k, v) in &inv.env {
            cmd.env(k, v);
        }
        if let Some(cwd) = &inv.cwd {
            cmd.current_dir(cwd.as_str());
        }
        cmd.stdin(if inv.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                cmd.pre_exec(|| {
                    nix::unistd::setpgid(
                        nix::unistd::Pid::from_raw(0),
                        nix::unistd::Pid::from_raw(0),
                    )
                    .map_err(std::io::Error::other)
                });
            }
        }

        let mut child = cmd.spawn().map_err(|e| XilinxError::ToolFailure {
            program: inv.program.clone(),
            code: -1,
            stderr: format!("spawn failed: {e}"),
        })?;

        if let (Some(bytes), Some(mut stdin)) = (inv.stdin.as_ref(), child.stdin.take()) {
            stdin.write_all(bytes)?;
        }

        if let Some(timeout) = inv.timeout {
            let deadline = std::time::Instant::now() + timeout;
            if let Some(status) = wait_with_deadline(&mut child, deadline)? {
                let mut stdout = String::new();
                let mut stderr = String::new();
                if let Some(mut o) = child.stdout.take() {
                    o.read_to_string(&mut stdout)?;
                }
                if let Some(mut e) = child.stderr.take() {
                    e.read_to_string(&mut stderr)?;
                }
                return match status.code() {
                    Some(code) => Ok(ToolOutput {
                        exit_code: code,
                        stdout,
                        stderr,
                    }),
                    None => Err(XilinxError::ToolSignaled {
                        program: inv.program.clone(),
                    }),
                };
            }
            #[cfg(unix)]
            {
                let pid = child.id();
                let process_group_id = nix::unistd::Pid::from_raw(pid.cast_signed());
                let _ =
                    nix::sys::signal::killpg(process_group_id, nix::sys::signal::Signal::SIGKILL);
            }
            #[cfg(not(unix))]
            {
                let _ = child.kill();
            }
            let _ = child.wait();
            return Err(XilinxError::ToolTimeout {
                program: inv.program.clone(),
                timeout_secs: timeout.as_secs(),
            });
        }

        let output = child.wait_with_output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        match output.status.code() {
            Some(code) => Ok(ToolOutput {
                exit_code: code,
                stdout,
                stderr,
            }),
            None => Err(XilinxError::ToolSignaled {
                program: inv.program.clone(),
            }),
        }
    }
}

/// Mock tool runner for unit tests. Responses are matched strictly on
/// `(program, args)` (FIFO within a matching group). Attached download
/// payloads are written to the file-system before `run` returns.
pub struct MockToolRunner {
    responses: Mutex<Vec<Response>>,
    calls: Mutex<Vec<ToolInvocation>>,
}

struct Response {
    program: String,
    args: Option<Vec<String>>,
    result: Result<ToolOutput>,
    downloads: HashMap<Utf8PathBuf, Vec<u8>>,
}

impl MockToolRunner {
    pub fn new() -> Self {
        Self {
            responses: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Queue a canned successful response for any next call to `program`.
    pub fn push_ok(&self, program: impl Into<String>, output: ToolOutput) {
        self.responses.lock().unwrap().push(Response {
            program: program.into(),
            args: None,
            result: Ok(output),
            downloads: HashMap::new(),
        });
    }

    /// Queue a canned response that only matches exact `(program, args)`.
    pub fn push_ok_for(&self, program: impl Into<String>, args: Vec<String>, output: ToolOutput) {
        self.responses.lock().unwrap().push(Response {
            program: program.into(),
            args: Some(args),
            result: Ok(output),
            downloads: HashMap::new(),
        });
    }

    /// Queue a canned error response. Lets producer tests trigger any
    /// `XilinxError` variant (not just `ToolFailure`) so the error
    /// coverage check exercises real `ToolRunner::run` returns.
    pub fn push_err(&self, program: impl Into<String>, err: XilinxError) {
        self.responses.lock().unwrap().push(Response {
            program: program.into(),
            args: None,
            result: Err(err),
            downloads: HashMap::new(),
        });
    }

    pub fn push_failure(&self, program: impl Into<String>, code: i32, stderr: impl Into<String>) {
        let program = program.into();
        let stderr = stderr.into();
        self.responses.lock().unwrap().push(Response {
            program: program.clone(),
            args: None,
            result: Err(XilinxError::ToolFailure {
                program,
                code,
                stderr,
            }),
            downloads: HashMap::new(),
        });
    }

    pub fn attach_download(&self, path: impl Into<Utf8PathBuf>, bytes: impl Into<Vec<u8>>) {
        let mut rs = self.responses.lock().unwrap();
        let last = rs
            .last_mut()
            .expect("attach_download called with no response queued");
        last.downloads.insert(path.into(), bytes.into());
    }

    pub fn calls(&self) -> Vec<ToolInvocation> {
        self.calls.lock().unwrap().clone()
    }
}

impl Default for MockToolRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRunner for MockToolRunner {
    fn run(&self, inv: &ToolInvocation) -> Result<ToolOutput> {
        self.calls.lock().unwrap().push(inv.clone());
        let mut responses = self.responses.lock().unwrap();
        let idx = responses.iter().position(|r| {
            r.program == inv.program && r.args.as_ref().is_none_or(|args| args == &inv.args)
        });
        let Some(idx) = idx else {
            return Err(XilinxError::ToolFailure {
                program: inv.program.clone(),
                code: -1,
                stderr: format!(
                    "MockToolRunner: no response queued for ({}, {:?})",
                    inv.program, inv.args
                ),
            });
        };
        let resp = responses.remove(idx);
        for (path, bytes) in resp.downloads {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, bytes)?;
        }
        resp.result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_returns_queued_ok() {
        let runner = MockToolRunner::new();
        runner.push_ok(
            "vivado",
            ToolOutput {
                exit_code: 0,
                stdout: "ok".into(),
                stderr: String::new(),
            },
        );
        let inv = ToolInvocation::new("vivado").arg("-mode").arg("batch");
        let out = runner.run(&inv).unwrap();
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.stdout, "ok");
    }

    #[test]
    fn mock_propagates_tool_failure() {
        let runner = MockToolRunner::new();
        runner.push_failure("vitis_hls", 1, "transient: TCP connection closed");
        let err = runner.run(&ToolInvocation::new("vitis_hls")).unwrap_err();
        assert!(matches!(err, XilinxError::ToolFailure { code: 1, .. }));
    }

    #[test]
    fn mock_dispatches_by_exact_args_when_given() {
        let runner = MockToolRunner::new();
        runner.push_ok_for(
            "vivado",
            vec!["-mode".into(), "batch".into()],
            ToolOutput {
                exit_code: 0,
                stdout: "batch".into(),
                stderr: String::new(),
            },
        );
        runner.push_ok_for(
            "vivado",
            vec!["-version".into()],
            ToolOutput {
                exit_code: 0,
                stdout: "v".into(),
                stderr: String::new(),
            },
        );
        assert_eq!(
            runner
                .run(&ToolInvocation::new("vivado").arg("-version"))
                .unwrap()
                .stdout,
            "v"
        );
        assert_eq!(
            runner
                .run(&ToolInvocation::new("vivado").arg("-mode").arg("batch"))
                .unwrap()
                .stdout,
            "batch"
        );
    }

    #[test]
    fn mock_writes_attached_downloads() {
        let tmp = tempfile::tempdir().unwrap();
        let dl = crate::util::utf8(tmp.path().join("nested").join("out.txt"));
        let runner = MockToolRunner::new();
        runner.push_ok("vitis_hls", ToolOutput::default());
        runner.attach_download(&dl, b"hello".to_vec());
        runner.run(&ToolInvocation::new("vitis_hls")).unwrap();
        assert_eq!(std::fs::read(&dl).unwrap(), b"hello");
    }

    #[test]
    fn local_runner_echo_roundtrip() {
        let runner = LocalToolRunner::new();
        let inv = ToolInvocation::new("/bin/sh")
            .arg("-c")
            .arg("printf hi; printf err 1>&2; exit 3");
        let out = runner.run(&inv).unwrap();
        assert_eq!(out.exit_code, 3);
        assert_eq!(out.stdout, "hi");
        assert_eq!(out.stderr, "err");
    }

    #[test]
    fn local_runner_honors_timeout() {
        let runner = LocalToolRunner::new();
        let inv = ToolInvocation::new("/bin/sh")
            .arg("-c")
            .arg("sleep 5")
            .timeout(Duration::from_millis(100));
        let err = runner.run(&inv).unwrap_err();
        match err {
            XilinxError::ToolTimeout {
                program,
                timeout_secs: _,
            } => assert_eq!(program, "/bin/sh"),
            other => panic!("expected ToolTimeout, got {other:?}"),
        }
    }

    #[test]
    fn unified_hls_args_translates_dash_f_and_keeps_extras() {
        assert_eq!(
            unified_hls_args(&["-f".into(), "run_hls.tcl".into()]),
            ["--mode", "hls", "--tcl", "run_hls.tcl"]
        );
        assert_eq!(
            unified_hls_args(&["-l".into(), "log".into(), "-f".into(), "a.tcl".into()]),
            ["--mode", "hls", "-l", "log", "--tcl", "a.tcl"]
        );
        assert_eq!(unified_hls_args(&[]), ["--mode", "hls"]);
    }

    #[test]
    fn hls_unified_rewrite_requires_vitis_run_and_no_classic_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        // Neither tool: no rewrite (spawn fails loudly on the classic name).
        assert!(!hls_needs_unified_rewrite(root));
        // Unified CLI only (Vitis 2025.1+): rewrite.
        std::fs::write(bin.join("vitis-run"), "").unwrap();
        assert!(hls_needs_unified_rewrite(root));
        // Classic binary present (<= 2024.2): keep the tested entry point.
        std::fs::write(bin.join("vitis_hls"), "").unwrap();
        assert!(!hls_needs_unified_rewrite(root));
    }

    /// End-to-end: a tool root that only ships `vitis-run` (Vitis
    /// 2025.1+) must receive `--mode hls --tcl <file>` even though the
    /// invocation asks for classic `vitis_hls -f <file>`.
    #[cfg(unix)]
    #[test]
    fn local_runner_rewrites_hls_to_vitis_run_when_classic_is_gone() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Vitis");
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(
            root.join("settings64.sh"),
            format!("export PATH='{}':\"$PATH\"\n", bin.display()),
        )
        .unwrap();
        let vitis_run = bin.join("vitis-run");
        std::fs::write(&vitis_run, "#!/bin/sh\nprintf '%s' \"$*\"\n").unwrap();
        let mut perms = std::fs::metadata(&vitis_run).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&vitis_run, perms).unwrap();
        if !std::process::Command::new(&vitis_run)
            .status()
            .is_ok_and(|status| status.success())
        {
            eprintln!("skipping: temp dir does not allow executing scripts");
            return;
        }

        let inv = ToolInvocation::new("vitis_hls")
            .arg("-f")
            .arg("run_hls.tcl")
            .env("XILINX_HLS", "")
            .env("XILINX_VITIS", root.display().to_string());
        let out = LocalToolRunner::new().run(&inv).unwrap();
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.stdout, "--mode hls --tcl run_hls.tcl");
    }

    /// Bare program names must resolve via the caller's `PATH`.
    /// Without this, bare `vitis_hls` / `vivado` spawn calls fail on a
    /// configured local host because the child has no `PATH` to search.
    #[test]
    fn local_runner_inherits_parent_path_for_bare_programs() {
        let runner = LocalToolRunner::new();
        // `sh` resolves purely via PATH: if env_clear() were in
        // effect, this spawn would fail with ENOENT.
        let inv = ToolInvocation::new("sh").arg("-c").arg("printf ok");
        let out = runner
            .run(&inv)
            .expect("bare `sh` must resolve via inherited PATH");
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.stdout, "ok");
    }

    #[cfg(unix)]
    #[test]
    fn local_runner_activates_xilinx_settings_for_local_tools() {
        use std::os::unix::fs::PermissionsExt;

        // Some CI hosts mount the default temp dir noexec; the fake tools
        // below must be executable, so skip when the probe cannot run.
        let probe = tempfile::tempdir().unwrap();
        let probe_script = probe.path().join("probe.sh");
        std::fs::write(&probe_script, "#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = std::fs::metadata(&probe_script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&probe_script, perms).unwrap();
        let can_exec = std::process::Command::new(&probe_script)
            .status()
            .is_ok_and(|status| status.success());
        drop(probe);
        if !can_exec {
            eprintln!("skipping: temp dir does not allow executing scripts");
            return;
        }

        for (tool, root_env) in [
            ("vitis_hls", "XILINX_HLS"),
            ("vitis_hls", "XILINX_VITIS"),
            ("vivado", "XILINX_VIVADO"),
            ("vivado", "XILINX_VITIS"),
            ("v++", "XILINX_VITIS"),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().join("Xilinx Tools");
            let bin = root.join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(
                root.join("settings64.sh"),
                format!(
                    "export TAPA_SETTINGS_MARKER=activated\nexport PATH='{}':\"$PATH\"\n",
                    bin.display()
                ),
            )
            .unwrap();
            let executable = bin.join(tool);
            std::fs::write(
                &executable,
                "#!/bin/sh\nprintf '%s|%s' \"$TAPA_SETTINGS_MARKER\" \"$1\"\n",
            )
            .unwrap();
            let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&executable, permissions).unwrap();

            let mut inv = ToolInvocation::new(tool)
                .arg("argument with spaces")
                .env("XILINX_HLS", "")
                .env("XILINX_VITIS", "")
                .env("XILINX_VIVADO", "");
            inv.env
                .insert(root_env.to_string(), root.display().to_string());

            let out = LocalToolRunner::new()
                .run(&inv)
                .unwrap_or_else(|e| panic!("{root_env} did not activate {tool}: {e}"));
            assert_eq!(out.exit_code, 0, "{root_env} / {tool}");
            assert_eq!(
                out.stdout, "activated|argument with spaces",
                "{root_env} / {tool}"
            );
        }
    }

    #[test]
    fn local_runner_ignores_xilinx_roots_for_unrelated_tools() {
        let inv = ToolInvocation::new("sh").env("XILINX_HLS", "/definitely/missing");
        assert!(
            local_xilinx_settings(&inv).is_none(),
            "only Xilinx tool invocations should source settings64.sh"
        );
    }

    /// `ToolInvocation::env` entries must overlay (not replace) the
    /// inherited env — so a caller can set `XILINX_HLS=/opt/...` while
    /// still letting the child see `PATH`, `HOME`, etc.
    #[test]
    fn local_runner_invocation_env_overlays_parent() {
        // Guarantee the parent has a non-empty PATH to inherit; the
        // test runner always sets it, but assert explicitly.
        assert!(std::env::var_os("PATH").is_some());
        let runner = LocalToolRunner::new();
        let inv = ToolInvocation::new("/bin/sh")
            .arg("-c")
            .arg("printf '%s\n' \"$TAPA_PROBE_VAR\" && test -n \"$PATH\" && echo path-ok")
            .env("TAPA_PROBE_VAR", "from-inv");
        let out = runner.run(&inv).unwrap();
        assert_eq!(out.exit_code, 0);
        assert!(
            out.stdout.contains("from-inv"),
            "overlay lost: {}",
            out.stdout
        );
        assert!(
            out.stdout.contains("path-ok"),
            "inherited PATH lost: {}",
            out.stdout
        );
    }

    #[test]
    fn local_runner_preserves_args_with_spaces() {
        let runner = LocalToolRunner::new();
        let inv = ToolInvocation::new("/bin/sh")
            .arg("-c")
            .arg("printf '%s' \"$1\"")
            .arg("dummy")
            .arg("hello world");
        let out = runner.run(&inv).unwrap();
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.stdout, "hello world");
    }

    #[test]
    fn local_runner_preserves_args_with_single_quotes() {
        let runner = LocalToolRunner::new();
        let inv = ToolInvocation::new("/bin/sh")
            .arg("-c")
            .arg("printf '%s' \"$1\"")
            .arg("dummy")
            .arg("it's working");
        let out = runner.run(&inv).unwrap();
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.stdout, "it's working");
    }

    #[test]
    fn local_runner_timeout_fires_quickly() {
        let runner = LocalToolRunner::new();
        let inv = ToolInvocation::new("/bin/sh")
            .arg("-c")
            .arg("sleep 10")
            .timeout(Duration::from_millis(50));
        let start = std::time::Instant::now();
        let err = runner.run(&inv).unwrap_err();
        let elapsed = start.elapsed();
        assert!(matches!(err, XilinxError::ToolTimeout { program, .. } if program == "/bin/sh"));
        assert!(
            elapsed < Duration::from_secs(2),
            "timeout should fire quickly, took {elapsed:?}"
        );
    }
}
