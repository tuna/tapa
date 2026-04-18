//! Remote tool runner: tar-pipe uploads / downloads and remote
//! invocation through a shared `SshSession`.
//!
//! The live implementation (tar-pipe, env allowlist, reconnect via
//! `classify_ssh_error`) is deferred to the remote-execution
//! milestone. This module fixes the type shape the orchestrators and
//! the PyO3 wrapper compile against.

use std::sync::Arc;

use crate::error::{Result, XilinxError};
use crate::runtime::process::{ToolInvocation, ToolOutput, ToolRunner};
use crate::runtime::ssh::SshSession;

pub struct RemoteToolRunner {
    session: Arc<SshSession>,
}

impl RemoteToolRunner {
    pub fn new(session: Arc<SshSession>) -> Self {
        Self { session }
    }

    pub fn session(&self) -> &SshSession {
        &self.session
    }
}

impl ToolRunner for RemoteToolRunner {
    fn run(&self, _inv: &ToolInvocation) -> Result<ToolOutput> {
        // The live tar-pipe + reconnect path is wired up by the
        // remote-execution milestone; keep a typed error until then so
        // callers cannot silently get empty output.
        Err(XilinxError::RemoteTransfer(
            "RemoteToolRunner.run not yet implemented".into(),
        ))
    }
}

/// One-shot vendor header sync from the configured remote.
///
/// Ports `tapa/remote/vendor.py`. Not yet wired up; returns a typed
/// error so call sites don't silently get an empty cache directory.
pub fn sync_remote_vendor_includes(_session: &SshSession) -> Result<std::path::PathBuf> {
    Err(XilinxError::RemoteTransfer(
        "sync_remote_vendor_includes not yet implemented".into(),
    ))
}
