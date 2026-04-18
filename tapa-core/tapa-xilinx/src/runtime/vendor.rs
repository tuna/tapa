//! Vendor-include sync: port of `tapa/remote/vendor.py`.
//!
//! Exposes one public entry point,
//! [`sync_remote_vendor_includes`], plus a [`VendorRemoteFs`] trait
//! so unit tests drive the algorithm without a live `SshSession`.
//!
//! Split out of `runtime::remote` during the Phase-6 simplifier
//! pass (the combined file overran the plan's soft 500 LOC
//! guideline).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{Result, XilinxError};
use crate::runtime::remote::shell_quote;
use crate::runtime::ssh::SshSession;

/// Abstraction over the two remote operations the vendor sync
/// needs: running a shell command over SSH and capturing
/// stdout/stderr/exit-code, and streaming a remote directory's
/// contents into a local destination via tar-pipe.
pub trait VendorRemoteFs {
    /// Run `cmd` on the remote in a login-style shell. Returns
    /// `(exit_code, stdout_bytes, stderr_bytes)`.
    fn ssh_exec(&self, cmd: &str) -> Result<(i32, Vec<u8>, Vec<u8>)>;

    /// Stream the remote directory at `remote_path` into the local
    /// directory `local_dest` (created if missing). Equivalent to
    /// `ssh … tar -czf - -C remote_path . | tar -xzf - -C local_dest`.
    fn download_dir(&self, remote_path: &str, local_dest: &Path) -> Result<()>;
}

struct SshVendorFs<'a> {
    session: &'a SshSession,
}

impl VendorRemoteFs for SshVendorFs<'_> {
    fn ssh_exec(&self, cmd: &str) -> Result<(i32, Vec<u8>, Vec<u8>)> {
        let mut args = self.session.build_ssh_args();
        args.push(self.session.ssh_target());
        args.push(cmd.to_string());
        let out = Command::new("ssh").args(&args).output().map_err(|e| {
            XilinxError::RemoteTransfer(format!("spawn ssh exec: {e}"))
        })?;
        Ok((out.status.code().unwrap_or(-1), out.stdout, out.stderr))
    }

    fn download_dir(&self, remote_path: &str, local_dest: &Path) -> Result<()> {
        std::fs::create_dir_all(local_dest).map_err(|e| {
            XilinxError::RemoteTransfer(format!(
                "mkdir {}: {e}",
                local_dest.display()
            ))
        })?;
        let remote_cmd =
            format!("tar -czf - -C {} .", shell_quote(remote_path));
        let mut args = self.session.build_ssh_args();
        args.push(self.session.ssh_target());
        args.push(remote_cmd);
        let mut ssh = Command::new("ssh")
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                XilinxError::RemoteTransfer(format!(
                    "spawn ssh download: {e}"
                ))
            })?;
        let ssh_stdout = ssh.stdout.take().ok_or_else(|| {
            XilinxError::RemoteTransfer("ssh stdout lost".into())
        })?;
        let mut tar_local = Command::new("tar")
            .arg("-xzf")
            .arg("-")
            .arg("-C")
            .arg(local_dest)
            .stdin(Stdio::from(ssh_stdout))
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                XilinxError::RemoteTransfer(format!(
                    "spawn local tar -xz: {e}"
                ))
            })?;
        let tar_status = tar_local.wait().map_err(|e| {
            XilinxError::RemoteTransfer(format!("wait tar -xz: {e}"))
        })?;
        let ssh_status = ssh.wait().map_err(|e| {
            XilinxError::RemoteTransfer(format!("wait ssh download: {e}"))
        })?;
        if !ssh_status.success() {
            return Err(XilinxError::RemoteTransfer(format!(
                "remote tar -cz failed: {ssh_status}"
            )));
        }
        if !tar_status.success() {
            return Err(XilinxError::RemoteTransfer(format!(
                "local tar -xz failed: {tar_status}"
            )));
        }
        Ok(())
    }
}

/// Parse the `KEY=VAL` lines produced by the remote
/// `echo XILINX_HLS=$XILINX_HLS && echo XILINX_VITIS=$XILINX_VITIS`
/// probe. Empty values are dropped (matches the Python loader).
pub(crate) fn parse_remote_xilinx_paths(
    stdout: &str,
) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for line in stdout.lines() {
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim();
            if !v.is_empty() {
                out.insert(k.trim().to_string(), v.to_string());
            }
        }
    }
    out
}

/// Compute the deterministic cache directory under
/// `$XDG_CACHE_HOME/tapa/vendor-headers/<key>` where `<key>` is the
/// first 16 hex chars of `sha256(host:port:xilinx_settings)` (matches
/// `tapa/remote/vendor.py::_cache_key`).
pub(crate) fn vendor_cache_dir(
    host: &str,
    port: u16,
    xilinx_settings: &str,
) -> Result<PathBuf> {
    use sha2::{Digest, Sha256};
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .ok_or_else(|| {
            XilinxError::RemoteTransfer(
                "vendor cache dir: neither XDG_CACHE_HOME nor HOME is set".into(),
            )
        })?;
    let raw = format!("{host}:{port}:{xilinx_settings}");
    let hash = Sha256::digest(raw.as_bytes());
    let mut key = String::with_capacity(16);
    for b in &hash[..8] {
        use std::fmt::Write as _;
        let _ = write!(key, "{b:02x}");
    }
    Ok(base.join("tapa").join("vendor-headers").join(key))
}

/// Apply the macOS libc++ compatibility patch to
/// `<cache_dir>/include/etc/ap_*_special.h`. Replaces the forward-
/// declaration block (see `tapa/remote/vendor.py::_patch_vendor_headers_for_macos`)
/// with `#include <complex>`. Idempotent: writes a marker
/// `.patched_macos_complex` to skip on subsequent calls. On non-macOS
/// hosts this is a no-op.
pub(crate) fn apply_macos_vendor_patch(cache_dir: &Path) -> Result<()> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    let marker = cache_dir.join(".patched_macos_complex");
    if marker.is_file() {
        return Ok(());
    }
    let etc_dir = cache_dir.join("include").join("etc");
    if !etc_dir.is_dir() {
        return Ok(());
    }
    #[allow(
        clippy::trivial_regex,
        reason = "literal multi-line match ported verbatim from the \
                  Python counterpart for source-of-truth parity"
    )]
    let pattern = regex::Regex::new(concat!(
        r"// FIXME AP_AUTOCC cannot handle many standard headers, so declare instead of\n",
        r"// include\.\n",
        r"// #include <complex>\n",
        r"namespace std \{\n",
        r"template<typename _Tp> class complex;\n",
        r"\}",
    ))
    .expect("static macOS patch pattern must compile");
    let replacement = "#include <complex>";
    let mut any = false;
    for entry in std::fs::read_dir(&etc_dir).map_err(|e| {
        XilinxError::RemoteTransfer(format!(
            "read_dir {}: {e}",
            etc_dir.display()
        ))
    })? {
        let entry = entry.map_err(|e| {
            XilinxError::RemoteTransfer(format!("read_dir entry: {e}"))
        })?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !(name.starts_with("ap_") && name.ends_with("_special.h")) {
            continue;
        }
        let content = std::fs::read_to_string(&path).map_err(|e| {
            XilinxError::RemoteTransfer(format!(
                "read {}: {e}",
                path.display()
            ))
        })?;
        let new_content = pattern.replace(&content, replacement);
        if new_content != content {
            std::fs::write(&path, new_content.as_bytes()).map_err(|e| {
                XilinxError::RemoteTransfer(format!(
                    "write {}: {e}",
                    path.display()
                ))
            })?;
            any = true;
        }
    }
    if any {
        std::fs::write(&marker, b"patched\n").map_err(|e| {
            XilinxError::RemoteTransfer(format!(
                "write macOS patch marker: {e}"
            ))
        })?;
    }
    Ok(())
}

/// One-shot vendor header sync from the configured remote.
///
/// Ports `tapa/remote/vendor.py::sync_remote_vendor_includes`.
///
/// 1. Source the remote `xilinx_settings` script and read back
///    `XILINX_HLS` / `XILINX_VITIS`.
/// 2. Stream `$XILINX_HLS/include` into `<cache_dir>/include` via
///    tar-pipe.
/// 3. Glob `$XILINX_HLS/tps/lnx64/gcc-*/include` on the remote and
///    mirror each one under `<cache_dir>/tps/lnx64/gcc-*/include`.
/// 4. On macOS hosts, patch the `ap_*_special.h` headers.
///
/// The cache directory is keyed by
/// `sha256(host:port:xilinx_settings)[:16]` so distinct remote
/// toolchains don't collide. Writing a `.synced` marker makes the
/// function idempotent.
pub fn sync_remote_vendor_includes(session: &SshSession) -> Result<PathBuf> {
    let cfg = session.config();
    let xilinx_settings = cfg.xilinx_settings.clone().ok_or_else(|| {
        XilinxError::RemoteTransfer(
            "sync_remote_vendor_includes: remote xilinx_settings unset".into(),
        )
    })?;
    session.ensure_established()?;
    let fs = SshVendorFs { session };
    let cache_dir = vendor_cache_dir(&cfg.host, cfg.port, &xilinx_settings)?;
    sync_vendor_includes_impl(&fs, &xilinx_settings, &cache_dir)
}

/// Pure algorithm driving the vendor include sync, parameterized
/// over a [`VendorRemoteFs`] and an explicit cache root so unit
/// tests can exercise every branch without a live SSH session and
/// without racing on the process-wide `XDG_CACHE_HOME` env var.
pub fn sync_vendor_includes_impl<F: VendorRemoteFs>(
    fs: &F,
    xilinx_settings: &str,
    cache_dir: &Path,
) -> Result<PathBuf> {
    let cache_dir = cache_dir.to_path_buf();
    let marker = cache_dir.join(".synced");
    if marker.is_file() {
        apply_macos_vendor_patch(&cache_dir)?;
        return Ok(cache_dir);
    }

    // Probe remote XILINX_HLS / XILINX_VITIS.
    let probe = format!(
        "source {s} && echo XILINX_HLS=$XILINX_HLS && echo XILINX_VITIS=$XILINX_VITIS",
        s = shell_quote(xilinx_settings),
    );
    let (rc, stdout, stderr) = fs.ssh_exec(&probe)?;
    if rc != 0 {
        return Err(XilinxError::RemoteTransfer(format!(
            "probe xilinx_settings: exit {rc}: {}",
            String::from_utf8_lossy(&stderr).trim()
        )));
    }
    let paths = parse_remote_xilinx_paths(&String::from_utf8_lossy(&stdout));
    let xilinx_tool = paths
        .get("XILINX_HLS")
        .or_else(|| paths.get("XILINX_VITIS"))
        .cloned()
        .ok_or_else(|| {
            XilinxError::RemoteTransfer(
                "remote XILINX_HLS / XILINX_VITIS not set after sourcing xilinx_settings".into(),
            )
        })?;

    std::fs::create_dir_all(&cache_dir).map_err(|e| {
        XilinxError::RemoteTransfer(format!(
            "mkdir cache {}: {e}",
            cache_dir.display()
        ))
    })?;

    // Remove any stale macOS patch marker so the patch re-applies
    // after a fresh header download.
    let patch_marker = cache_dir.join(".patched_macos_complex");
    if patch_marker.exists() {
        std::fs::remove_file(&patch_marker).map_err(|e| {
            XilinxError::RemoteTransfer(format!(
                "remove stale patch marker: {e}"
            ))
        })?;
    }

    // Download include/.
    let remote_include = format!("{xilinx_tool}/include");
    let local_include = cache_dir.join("include");
    fs.download_dir(&remote_include, &local_include)?;

    // Glob remote tps/lnx64/gcc-*/include directories and mirror each
    // one under the same relative path locally.
    let ls_cmd = format!(
        "ls -d {xt}/tps/lnx64/gcc-*/include 2>/dev/null || true",
        xt = shell_quote(&xilinx_tool),
    );
    let (_, ls_out, _) = fs.ssh_exec(&ls_cmd)?;
    for remote_gcc_inc in String::from_utf8_lossy(&ls_out)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let rel = remote_gcc_inc
            .strip_prefix(&format!("{xilinx_tool}/"))
            .unwrap_or(remote_gcc_inc)
            .to_string();
        let local_gcc = cache_dir.join(&rel);
        fs.download_dir(remote_gcc_inc, &local_gcc)?;
    }

    apply_macos_vendor_patch(&cache_dir)?;
    std::fs::write(&marker, format!("{xilinx_tool}\n")).map_err(|e| {
        XilinxError::RemoteTransfer(format!("write synced marker: {e}"))
    })?;
    Ok(cache_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    type SshCannedResponse = (i32, Vec<u8>, Vec<u8>);

    /// Mock implementation driving the algorithm through canned
    /// responses. `ssh_exec_responses` is consumed FIFO; each call to
    /// `ssh_exec` pops one. `download_dir` writes a synthetic file
    /// tree into the destination so downstream logic (the macOS
    /// patch, the marker write) exercises real filesystem paths.
    struct MockFs {
        ssh_exec_responses: RefCell<VecDeque<SshCannedResponse>>,
        download_fail_on: Option<String>,
        recorded_downloads: RefCell<Vec<String>>,
        write_ap_special: bool,
    }

    impl MockFs {
        fn new(responses: Vec<SshCannedResponse>) -> Self {
            Self {
                ssh_exec_responses: RefCell::new(responses.into()),
                download_fail_on: None,
                recorded_downloads: RefCell::new(Vec::new()),
                write_ap_special: false,
            }
        }
    }

    impl VendorRemoteFs for MockFs {
        fn ssh_exec(&self, _cmd: &str) -> Result<(i32, Vec<u8>, Vec<u8>)> {
            self.ssh_exec_responses
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| {
                    XilinxError::RemoteTransfer(
                        "MockFs: no more canned ssh responses".into(),
                    )
                })
        }
        fn download_dir(
            &self,
            remote_path: &str,
            local_dest: &Path,
        ) -> Result<()> {
            self.recorded_downloads
                .borrow_mut()
                .push(remote_path.to_string());
            if self
                .download_fail_on
                .as_deref()
                .is_some_and(|frag| remote_path.contains(frag))
            {
                return Err(XilinxError::RemoteTransfer(format!(
                    "mock tar-pipe failed for {remote_path}"
                )));
            }
            std::fs::create_dir_all(local_dest).map_err(|e| {
                XilinxError::RemoteTransfer(format!(
                    "mock mkdir {}: {e}",
                    local_dest.display()
                ))
            })?;
            std::fs::write(local_dest.join(".mock_download"), remote_path)
                .map_err(|e| {
                    XilinxError::RemoteTransfer(format!("mock write: {e}"))
                })?;
            if self.write_ap_special && local_dest.ends_with("include") {
                let etc = local_dest.join("etc");
                std::fs::create_dir_all(&etc).unwrap();
                let body = concat!(
                    "// FIXME AP_AUTOCC cannot handle many standard headers, so declare instead of\n",
                    "// include.\n",
                    "// #include <complex>\n",
                    "namespace std {\n",
                    "template<typename _Tp> class complex;\n",
                    "}\n",
                    "struct rest_of_header {};\n",
                );
                std::fs::write(etc.join("ap_fixed_special.h"), body).unwrap();
            }
            Ok(())
        }
    }

    fn isolate_cache() -> (tempfile::TempDir, PathBuf) {
        let td = tempfile::tempdir().expect("tempdir");
        let key = td.path().join("tapa").join("vendor-headers").join("k");
        (td, key)
    }

    #[test]
    fn parse_remote_xilinx_paths_handles_mixed_lines() {
        let text = "XILINX_HLS=/opt/xilinx/hls\nXILINX_VITIS=\nnoise";
        let m = parse_remote_xilinx_paths(text);
        assert_eq!(m.get("XILINX_HLS").unwrap(), "/opt/xilinx/hls");
        assert!(!m.contains_key("XILINX_VITIS"));
    }

    #[test]
    fn cache_dir_is_deterministic_and_keyed() {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = ENV_LOCK.lock().unwrap();
        let td = tempfile::tempdir().expect("tempdir");
        let prev = std::env::var_os("XDG_CACHE_HOME");
        std::env::set_var("XDG_CACHE_HOME", td.path());
        let a = vendor_cache_dir("h1", 22, "/opt/settings64.sh").unwrap();
        let b = vendor_cache_dir("h1", 22, "/opt/settings64.sh").unwrap();
        let c = vendor_cache_dir("h2", 22, "/opt/settings64.sh").unwrap();
        if let Some(p) = prev {
            std::env::set_var("XDG_CACHE_HOME", p);
        } else {
            std::env::remove_var("XDG_CACHE_HOME");
        }
        drop(td);
        assert_eq!(a, b);
        assert_ne!(a, c);
        let key = a.file_name().unwrap().to_string_lossy();
        assert_eq!(key.len(), 16);
        assert!(key.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn happy_path_downloads_include_and_gcc_dirs() {
        let (_td, cache) = isolate_cache();
        let mock = MockFs::new(vec![
            (
                0,
                b"XILINX_HLS=/opt/xilinx/hls\nXILINX_VITIS=\n".to_vec(),
                Vec::new(),
            ),
            (
                0,
                b"/opt/xilinx/hls/tps/lnx64/gcc-6.2.0/include\n".to_vec(),
                Vec::new(),
            ),
        ]);
        let out =
            sync_vendor_includes_impl(&mock, "/opt/settings64.sh", &cache)
                .expect("sync must succeed");
        assert_eq!(out, cache);
        assert!(cache.join("include").join(".mock_download").is_file());
        assert!(cache
            .join("tps/lnx64/gcc-6.2.0/include")
            .join(".mock_download")
            .is_file());
        assert!(cache.join(".synced").is_file());
        let dls = mock.recorded_downloads.borrow().clone();
        assert_eq!(
            dls,
            vec![
                "/opt/xilinx/hls/include".to_string(),
                "/opt/xilinx/hls/tps/lnx64/gcc-6.2.0/include".to_string(),
            ]
        );
    }

    #[test]
    fn missing_remote_xilinx_paths_surfaces_typed_error() {
        let (_td, cache) = isolate_cache();
        let mock = MockFs::new(vec![(
            0,
            b"XILINX_HLS=\nXILINX_VITIS=\n".to_vec(),
            Vec::new(),
        )]);
        let err =
            sync_vendor_includes_impl(&mock, "/opt/settings64.sh", &cache)
                .expect_err("must error when no tool paths");
        match err {
            XilinxError::RemoteTransfer(msg) => {
                assert!(msg.contains("XILINX_HLS"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
        assert!(!cache.join(".synced").exists());
    }

    #[test]
    fn probe_nonzero_exit_surfaces_typed_error() {
        let (_td, cache) = isolate_cache();
        let mock = MockFs::new(vec![(
            127,
            Vec::new(),
            b"settings64.sh: not found".to_vec(),
        )]);
        let err =
            sync_vendor_includes_impl(&mock, "/opt/settings64.sh", &cache)
                .expect_err("probe nonzero must error");
        match err {
            XilinxError::RemoteTransfer(msg) => {
                assert!(msg.contains("probe"));
                assert!(msg.contains("127"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn tar_pipe_failure_on_include_surfaces_typed_error() {
        let (_td, cache) = isolate_cache();
        let mut mock = MockFs::new(vec![(
            0,
            b"XILINX_HLS=/opt/xilinx/hls\n".to_vec(),
            Vec::new(),
        )]);
        mock.download_fail_on = Some("/include".to_string());
        let err =
            sync_vendor_includes_impl(&mock, "/opt/settings64.sh", &cache)
                .expect_err("download failure must error");
        assert!(matches!(err, XilinxError::RemoteTransfer(_)));
    }

    #[test]
    fn idempotent_second_call_skips_runner() {
        let (_td, cache) = isolate_cache();
        let mock = MockFs::new(vec![
            (0, b"XILINX_HLS=/opt/xilinx/hls\n".to_vec(), Vec::new()),
            (0, b"".to_vec(), Vec::new()),
        ]);
        let cache1 =
            sync_vendor_includes_impl(&mock, "/opt/settings64.sh", &cache)
                .unwrap();
        let remaining_before = mock.ssh_exec_responses.borrow().len();
        let cache2 =
            sync_vendor_includes_impl(&mock, "/opt/settings64.sh", &cache)
                .unwrap();
        assert_eq!(cache1, cache2);
        assert_eq!(
            remaining_before,
            mock.ssh_exec_responses.borrow().len()
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_patch_rewrites_ap_special_headers() {
        let (_td, cache) = isolate_cache();
        let mut mock = MockFs::new(vec![
            (0, b"XILINX_HLS=/opt/xilinx/hls\n".to_vec(), Vec::new()),
            (0, b"".to_vec(), Vec::new()),
        ]);
        mock.write_ap_special = true;
        let out =
            sync_vendor_includes_impl(&mock, "/opt/settings64.sh", &cache)
                .unwrap();
        let patched = std::fs::read_to_string(
            out.join("include").join("etc").join("ap_fixed_special.h"),
        )
        .expect("header");
        assert!(patched.contains("#include <complex>"));
        assert!(!patched.contains("template<typename _Tp> class complex"));
        assert!(out.join(".patched_macos_complex").is_file());
    }
}
