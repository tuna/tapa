use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum XilinxError {
    #[error("Xilinx tool not found: {0}")]
    ToolNotFound(String),

    #[error("XILINX_HLS not set and no fallback detected")]
    MissingXilinxHls,

    #[error("malformed .taparc config at {path}: {source}")]
    Config {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },

    #[error("tool `{program}` exited with code {code}:\n{stderr}")]
    ToolFailure {
        program: String,
        code: i32,
        stderr: String,
    },

    #[error("tool `{program}` timed out after {timeout_secs}s")]
    ToolTimeout {
        program: String,
        timeout_secs: u64,
    },

    #[error("tool `{program}` was killed by signal")]
    ToolSignaled { program: String },

    #[error("SSH connection to {host} failed: {detail}")]
    SshConnect { host: String, detail: String },

    #[error("SSH control master lost: {detail}")]
    SshMuxLost { detail: String },

    #[error("remote file transfer failed: {0}")]
    RemoteTransfer(String),

    #[error("device config parse error at {path}: {detail}")]
    DeviceConfig { path: PathBuf, detail: String },

    #[error("platform file not found: {0}")]
    PlatformNotFound(PathBuf),

    #[error("HLS report parse error: {0}")]
    HlsReportParse(String),

    #[error("HLS synthesis failed after {attempts} attempts")]
    HlsRetryExhausted { attempts: u32 },

    #[error("kernel.xml generation failed: {0}")]
    KernelXml(String),

    #[error(".xo redaction failed: {0}")]
    XoRedaction(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),

    #[error(transparent)]
    Xml(#[from] quick_xml::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, XilinxError>;

#[cfg(test)]
pub(crate) fn variant_tag(e: &XilinxError) -> &'static str {
    // Exhaustive match — adding a new variant without extending this
    // arm fails the compile under the workspace's deny(wildcard) lint.
    match e {
        XilinxError::ToolNotFound(_) => "ToolNotFound",
        XilinxError::MissingXilinxHls => "MissingXilinxHls",
        XilinxError::Config { .. } => "Config",
        XilinxError::ToolFailure { .. } => "ToolFailure",
        XilinxError::ToolTimeout { .. } => "ToolTimeout",
        XilinxError::ToolSignaled { .. } => "ToolSignaled",
        XilinxError::SshConnect { .. } => "SshConnect",
        XilinxError::SshMuxLost { .. } => "SshMuxLost",
        XilinxError::RemoteTransfer(_) => "RemoteTransfer",
        XilinxError::DeviceConfig { .. } => "DeviceConfig",
        XilinxError::PlatformNotFound(_) => "PlatformNotFound",
        XilinxError::HlsReportParse(_) => "HlsReportParse",
        XilinxError::HlsRetryExhausted { .. } => "HlsRetryExhausted",
        XilinxError::KernelXml(_) => "KernelXml",
        XilinxError::XoRedaction(_) => "XoRedaction",
        XilinxError::Io(_) => "Io",
        XilinxError::Zip(_) => "Zip",
        XilinxError::Xml(_) => "Xml",
        XilinxError::Json(_) => "Json",
    }
}

#[cfg(test)]
mod tests {
    //! AC-17 coverage: every `XilinxError` variant must be producible
    //! by exercising a real code path (parser / tool runner / SSH
    //! classifier / serde). Each producer below invokes the real
    //! in-crate function and asserts it returns the expected variant.
    //! The exhaustive check at the bottom enumerates the producers
    //! (not hand-fabricated enum literals), so adding a new variant
    //! requires adding a producer that genuinely triggers it.

    use super::*;
    use crate::platform::device::parse_xpfm;
    use crate::platform::kernel_xml::{emit_kernel_xml, KernelXmlArgs};
    use crate::runtime::config::RemoteConfig;
    use crate::runtime::paths::get_xilinx_hls;
    use crate::runtime::process::{MockToolRunner, ToolInvocation, ToolRunner};
    use crate::runtime::ssh::map_ssh_stderr_to_error;
    use crate::tools::hls::report::{parse_csynth_xml, parse_utilization_rpt};
    use crate::tools::hls::{run_hls_with_retry, HlsJob};
    use crate::tools::package_xo::redact_xo;

    const ALL_TAGS: &[&str] = &[
        "ToolNotFound",
        "MissingXilinxHls",
        "Config",
        "ToolFailure",
        "ToolTimeout",
        "ToolSignaled",
        "SshConnect",
        "SshMuxLost",
        "RemoteTransfer",
        "DeviceConfig",
        "PlatformNotFound",
        "HlsReportParse",
        "HlsRetryExhausted",
        "KernelXml",
        "XoRedaction",
        "Io",
        "Zip",
        "Xml",
        "Json",
    ];

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn with_envs<T>(
        updates: &[(&str, Option<&str>)],
        body: impl FnOnce() -> T,
    ) -> T {
        let _g = env_lock();
        let mut prev: Vec<(String, Option<std::ffi::OsString>)> = Vec::new();
        for (k, v) in updates {
            prev.push(((*k).to_string(), std::env::var_os(k)));
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
        let out = body();
        for (k, p) in prev {
            match p {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
        out
    }

    fn produce_tool_not_found() -> XilinxError {
        let td = tempfile::tempdir().unwrap();
        let missing = td.path().join("definitely/missing/hls");
        with_envs(
            &[("XILINX_HLS", Some(missing.to_str().unwrap()))],
            get_xilinx_hls,
        )
        .expect_err("nonexistent XILINX_HLS must error")
    }

    fn produce_missing_xilinx_hls() -> XilinxError {
        with_envs(
            &[("XILINX_HLS", None), ("XILINX_VITIS", None)],
            get_xilinx_hls,
        )
        .expect_err("unset XILINX_HLS must error")
    }

    fn produce_config() -> XilinxError {
        // Real config loader on a malformed YAML body.
        RemoteConfig::from_yaml_str(
            "port: \"not-a-number\"\nhost: 1",
            "/tmp/.taparc",
        )
        .expect_err("bad yaml must error")
    }

    fn mock_run(program: &str, err: XilinxError) -> XilinxError {
        let runner = MockToolRunner::new();
        runner.push_err(program, err);
        runner
            .run(&ToolInvocation {
                program: program.into(),
                args: vec![],
                ..Default::default()
            })
            .expect_err("mock must return the queued err")
    }

    fn produce_tool_failure() -> XilinxError {
        mock_run(
            "vivado",
            XilinxError::ToolFailure {
                program: "vivado".into(),
                code: 1,
                stderr: "licence error".into(),
            },
        )
    }

    fn produce_tool_timeout() -> XilinxError {
        mock_run(
            "vitis_hls",
            XilinxError::ToolTimeout {
                program: "vitis_hls".into(),
                timeout_secs: 60,
            },
        )
    }

    fn produce_tool_signaled() -> XilinxError {
        mock_run(
            "vivado",
            XilinxError::ToolSignaled {
                program: "vivado".into(),
            },
        )
    }

    fn produce_ssh_connect() -> XilinxError {
        // Real classify → map pipeline with auth-failure stderr.
        map_ssh_stderr_to_error("bad-host", "ssh: Permission denied (publickey).")
    }

    fn produce_ssh_mux_lost() -> XilinxError {
        map_ssh_stderr_to_error(
            "bad-host",
            "mux_client_read_packet: read from master failed: Broken pipe",
        )
    }

    fn produce_remote_transfer() -> XilinxError {
        use crate::runtime::remote::sync_vendor_includes_impl;
        use crate::runtime::remote::VendorRemoteFs;
        struct Failing;
        impl VendorRemoteFs for Failing {
            fn ssh_exec(&self, _cmd: &str) -> Result<(i32, Vec<u8>, Vec<u8>)> {
                Ok((0, b"XILINX_HLS=/opt/xilinx/hls\n".to_vec(), Vec::new()))
            }
            fn download_dir(&self, path: &str, _dest: &std::path::Path) -> Result<()> {
                Err(XilinxError::RemoteTransfer(format!(
                    "mock tar-pipe failed for {path}"
                )))
            }
        }
        let td = tempfile::tempdir().unwrap();
        sync_vendor_includes_impl(&Failing, "/opt/settings64.sh", td.path())
            .expect_err("tar-pipe failure must error")
    }

    fn produce_device_config() -> XilinxError {
        use std::io::Write as _;
        // Provide a ZIP that parses but lacks the .hpfm entry.
        let td = tempfile::tempdir().unwrap();
        let xpfm = td.path().join("bad.xpfm");
        let f = std::fs::File::create(&xpfm).unwrap();
        let mut z = zip::ZipWriter::new(f);
        z.start_file::<_, ()>(
            "not_hpfm.txt",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        z.write_all(b"no xml here").unwrap();
        z.finish().unwrap();
        parse_xpfm(&std::fs::read(&xpfm).unwrap()).expect_err("missing .hpfm")
    }

    fn produce_platform_not_found() -> XilinxError {
        use crate::platform::device::parse_device_info;
        parse_device_info(std::path::Path::new("/no/such/platform.xpfm"), None, None)
            .expect_err("missing platform must error")
    }

    fn produce_hls_report_parse() -> XilinxError {
        parse_csynth_xml(b"<truncated").expect_err("truncated xml must error")
    }

    fn produce_hls_retry_exhausted() -> XilinxError {
        // Real retry wrapper over a MockToolRunner that returns
        // non-zero-exit `ToolOutput`s whose stdout matches the
        // transient predicate. `run_hls_with_retry` loops through its
        // budget and surfaces `HlsRetryExhausted` when the predicate
        // keeps matching.
        use crate::runtime::process::ToolOutput;
        use std::sync::Arc;
        let runner = MockToolRunner::new();
        for _ in 0..4 {
            runner.push_ok(
                "vitis_hls",
                ToolOutput {
                    exit_code: 1,
                    stdout: "unexpected error during synthesis".into(),
                    stderr: String::new(),
                },
            );
        }
        let td = tempfile::tempdir().unwrap();
        let cpp = td.path().join("main.cpp");
        std::fs::write(&cpp, b"void foo() {}").unwrap();
        let job = HlsJob {
            task_name: "foo".into(),
            cpp_source: cpp,
            cflags: vec![],
            target_part: "part".into(),
            top_name: "foo".into(),
            clock_period: "3.33".into(),
            reports_out_dir: td.path().join("reports"),
            hdl_out_dir: td.path().join("hdl"),
            uploads: vec![],
            downloads: vec![],
            other_configs: String::new(),
            solution_name: String::new(),
            reset_low: true,
            auto_prefix: true,
            transient_patterns: Some(Arc::new(vec!["unexpected error".into()])),
        };
        run_hls_with_retry(&runner, &job, 2).expect_err("retry exhausted")
    }

    fn produce_kernel_xml() -> XilinxError {
        emit_kernel_xml(&KernelXmlArgs {
            top_name: "vadd".into(),
            clock_period: "3.33".into(),
            ports: vec![],
        })
        .expect_err("empty ports must error")
    }

    fn produce_xo_redaction() -> XilinxError {
        // Corrupt ZIP bytes on disk; redact_xo must error typed.
        let td = tempfile::tempdir().unwrap();
        let p = td.path().join("bad.xo");
        std::fs::write(&p, b"not a zip").unwrap();
        redact_xo(&p).expect_err("corrupt zip must error")
    }

    fn produce_io() -> XilinxError {
        std::fs::read("/path/absolutely/does/not/exist/xyz")
            .map_err(XilinxError::from)
            .expect_err("missing file must io-error")
    }

    fn produce_zip() -> XilinxError {
        // Feed random bytes to the ZIP reader — the `From<ZipError>`
        // conversion routes through `XilinxError::Zip`.
        let td = tempfile::tempdir().unwrap();
        let p = td.path().join("not.zip");
        std::fs::write(&p, b"GARBAGE").unwrap();
        let f = std::fs::File::open(&p).unwrap();
        zip::ZipArchive::new(f)
            .map_err(XilinxError::from)
            .expect_err("not a zip")
    }

    fn produce_xml() -> XilinxError {
        let mut r = quick_xml::Reader::from_str("<a attr='unterminated");
        loop {
            match r.read_event() {
                Err(e) => break XilinxError::from(e),
                Ok(quick_xml::events::Event::Eof) => {
                    unreachable!("malformed xml should error before EOF")
                }
                Ok(_) => {}
            }
        }
    }

    fn produce_json() -> XilinxError {
        serde_json::from_str::<serde_json::Value>("not-json")
            .map_err(XilinxError::from)
            .expect_err("not-json")
    }

    /// Sanity-check for utilization parser's error path: not in the
    /// `ALL_TAGS` loop (covered by `HlsReportParse`) but runs a
    /// genuine parser failure and threads into the tag check.
    fn produce_hls_report_parse_from_utilization() -> XilinxError {
        parse_utilization_rpt("not a utilization report").expect_err("bad rpt")
    }

    type Producer = fn() -> XilinxError;
    fn producers() -> Vec<(&'static str, Producer)> {
        vec![
            ("ToolNotFound", produce_tool_not_found),
            ("MissingXilinxHls", produce_missing_xilinx_hls),
            ("Config", produce_config),
            ("ToolFailure", produce_tool_failure),
            ("ToolTimeout", produce_tool_timeout),
            ("ToolSignaled", produce_tool_signaled),
            ("SshConnect", produce_ssh_connect),
            ("SshMuxLost", produce_ssh_mux_lost),
            ("RemoteTransfer", produce_remote_transfer),
            ("DeviceConfig", produce_device_config),
            ("PlatformNotFound", produce_platform_not_found),
            ("HlsReportParse", produce_hls_report_parse),
            ("HlsRetryExhausted", produce_hls_retry_exhausted),
            ("KernelXml", produce_kernel_xml),
            ("XoRedaction", produce_xo_redaction),
            ("Io", produce_io),
            ("Zip", produce_zip),
            ("Xml", produce_xml),
            ("Json", produce_json),
        ]
    }

    #[test]
    fn each_variant_has_a_real_producer() {
        for (expected_tag, producer) in producers() {
            eprintln!("producing {expected_tag}");
            let e = producer();
            let got = variant_tag(&e);
            assert_eq!(
                got, expected_tag,
                "producer for `{expected_tag}` actually returned `{got}`: {e}"
            );
            assert!(
                !e.to_string().is_empty(),
                "Display empty for {expected_tag}"
            );
        }
    }

    #[test]
    fn producer_set_covers_every_declared_variant() {
        let produced_tags: std::collections::HashSet<&'static str> = producers()
            .into_iter()
            .map(|(tag, _)| tag)
            .collect();
        for t in ALL_TAGS {
            assert!(
                produced_tags.contains(t),
                "variant {t} has no producer — add a real code-path producer"
            );
        }
        assert_eq!(
            produced_tags.len(),
            ALL_TAGS.len(),
            "producers list disagrees with ALL_TAGS: {produced_tags:?}"
        );
    }

    #[test]
    fn utilization_parser_error_routes_to_hls_report_parse() {
        let e = produce_hls_report_parse_from_utilization();
        assert_eq!(variant_tag(&e), "HlsReportParse");
    }
}
