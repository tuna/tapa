//! Subprocess abstraction: the `ToolRunner` trait plus the local
//! `std::process::Command`-backed implementation and a test-only mock.
//!
//! Every tool wrapper speaks through this trait so unit tests never
//! need `vitis_hls` or `vivado` on the host.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use crate::error::{Result, XilinxError};

#[derive(Debug, Clone, Default)]
pub struct ToolInvocation {
    pub program: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub stdin: Option<Vec<u8>>,
    pub cwd: Option<PathBuf>,
    pub uploads: Vec<PathBuf>,
    pub downloads: Vec<PathBuf>,
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

    /// Stage a sub-tree from the runner's execution scope back onto
    /// the local filesystem. Callers pass a `relative_from_cwd`
    /// path (e.g. `project/<solution>/syn`) and a `local_root`;
    /// after a successful return the tree lives at
    /// `local_root.join(relative_from_cwd)` with files in place.
    ///
    /// Default: no-op. `LocalToolRunner` already wrote directly
    /// under `ToolInvocation::cwd`; `MockToolRunner` leaves the
    /// simulated stage untouched. `RemoteToolRunner` overrides to
    /// tar-pipe the remote work directory's subtree into
    /// `local_root`.
    fn harvest(
        &self,
        _relative_from_cwd: &std::path::Path,
        _local_root: &std::path::Path,
    ) -> Result<()> {
        Ok(())
    }
}

/// Local subprocess runner with environment allowlisting and optional
/// per-invocation timeout.
///
/// `env_clear()` is applied so only variables explicitly listed in
/// `ToolInvocation::env` reach the child — matches the Python remote
/// layer's env allowlist behavior. When `ToolInvocation::timeout` is
/// set, the child is killed on expiry and the call returns
/// `XilinxError::ToolTimeout`.
#[derive(Debug, Default)]
pub struct LocalToolRunner;

impl LocalToolRunner {
    pub const fn new() -> Self {
        Self
    }
}

fn wait_with_deadline(
    child: &mut std::process::Child,
    deadline: std::time::Instant,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    let poll = Duration::from_millis(20);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if std::time::Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(poll);
    }
}

impl ToolRunner for LocalToolRunner {
    fn run(&self, inv: &ToolInvocation) -> Result<ToolOutput> {
        use std::io::{Read, Write};
        use std::process::{Command, Stdio};

        let mut cmd = Command::new(&inv.program);
        cmd.args(&inv.args);
        cmd.env_clear();
        for (k, v) in &inv.env {
            cmd.env(k, v);
        }
        if let Some(cwd) = &inv.cwd {
            cmd.current_dir(cwd);
        }
        cmd.stdin(if inv.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

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
            let _ = child.kill();
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
    downloads: HashMap<PathBuf, Vec<u8>>,
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
    pub fn push_ok_for(
        &self,
        program: impl Into<String>,
        args: Vec<String>,
        output: ToolOutput,
    ) {
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

    pub fn attach_download(&self, path: impl Into<PathBuf>, bytes: impl Into<Vec<u8>>) {
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
            r.program == inv.program
                && r.args.as_ref().is_none_or(|args| args == &inv.args)
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
        let dl = tmp.path().join("nested").join("out.txt");
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
}
