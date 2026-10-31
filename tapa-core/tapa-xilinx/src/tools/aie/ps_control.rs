//! Cross-build of the small PS kernel that controls a TAPA graph through
//! XRT on a VCK5000's APU.

use camino::Utf8PathBuf;
use typed_builder::TypedBuilder;

use crate::error::{Result, XilinxError};
use crate::runtime::process::{ToolInvocation, ToolRunner};
use crate::tools::tool_failure;
use crate::ToolValue;

const SYSROOT: &str = "sysroots/cortexa72-cortexa53-xilinx-linux";
const FIRMWARE_SDK: &str = "/lib/firmware/xilinx/sysroot/sdk.sh";
const SOURCE: &str = include_str!("ps_control.cpp");

/// The sysroot used to link the APU control kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiePsSdk {
    /// A common-image root containing `sysroots/cortexa72-cortexa53-xilinx-linux`.
    Sysroot(Utf8PathBuf),
    /// A self-extracting SDK installer, normally the VCK5000 firmware's `sdk.sh`.
    Installer(Utf8PathBuf),
}

impl AiePsSdk {
    /// Prefer an explicitly configured common image, then the card firmware's
    /// matching SDK installer.
    #[must_use]
    pub fn from_environment() -> Self {
        match std::env::var_os("EDGE_COMMON_SW").filter(|path| !path.is_empty()) {
            Some(path) => Self::Sysroot(
                Utf8PathBuf::from_path_buf(path.into())
                    .expect("EDGE_COMMON_SW must be valid UTF-8 when it names an AIE sysroot"),
            ),
            None => Self::Installer(FIRMWARE_SDK.into()),
        }
    }
}

/// Whether this platform controls its AIE graph through an APU PS kernel.
#[must_use]
pub fn required(platform: &str) -> bool {
    platform.contains("vck5000")
}

/// One design-specific APU graph-control shared-library build.
#[derive(Debug, Clone, TypedBuilder)]
pub struct AiePsControlJob {
    pub graph_name: String,
    pub sdk: AiePsSdk,
    /// Generated sources and, when needed, the extracted SDK. Not downloaded.
    pub work_dir: Utf8PathBuf,
    pub output: Utf8PathBuf,
}

fn source(graph_name: &str) -> Result<String> {
    if !matches!(graph_name, "tapa_aie" | "tapa_aie_forever") {
        return Err(XilinxError::AieGraph(format!(
            "unsupported PS-control graph name `{graph_name}`"
        )));
    }
    Ok(SOURCE.replace("@GRAPH_NAME@", graph_name))
}

fn run(runner: &dyn ToolRunner, inv: &ToolInvocation, program: &str) -> Result<()> {
    let out = runner.run(inv)?;
    if out.exit_code != 0 {
        return Err(tool_failure(program, out));
    }
    Ok(())
}

/// Build the control kernel and leave its shared library at `job.output`.
pub fn compile(runner: &dyn ToolRunner, job: &AiePsControlJob) -> Result<()> {
    let source_text = source(&job.graph_name)?;
    fs_err::create_dir_all(&job.work_dir)?;
    if let Some(parent) = job.output.parent() {
        fs_err::create_dir_all(parent)?;
    }
    let source_path = job.work_dir.join("tapa_aie_control.cpp");
    fs_err::write(&source_path, source_text)?;

    let sysroot = match &job.sdk {
        AiePsSdk::Sysroot(root) => root.join(SYSROOT),
        AiePsSdk::Installer(installer) => {
            let sdk_dir = job.work_dir.join("sdk");
            let mut inv = ToolInvocation::new("sh")
                .arg(ToolValue::path(installer))
                .arg("-y")
                .arg("-d")
                .arg(ToolValue::path(&sdk_dir));
            inv.cwd = Some(job.work_dir.clone());
            run(runner, &inv, "sh")?;
            sdk_dir.join(SYSROOT)
        }
    };

    let mut inv = ToolInvocation::new("aarch64-linux-gnu-g++")
        .arg("-std=c++17")
        .arg("-O2")
        .arg("-g")
        .arg("-fPIC")
        .arg("-fvisibility=hidden")
        .arg("-D__PS_ENABLE_AIE__")
        .arg(format!("--sysroot={sysroot}"))
        .arg(format!("-I{sysroot}/usr/include/xrt"))
        .arg(ToolValue::path(&source_path))
        .arg("-shared")
        .arg("-lxrt_coreutil")
        .arg("-lpthread")
        .arg("-lrt")
        .arg("-ldl")
        .arg("-lcrypt")
        .arg("-lstdc++")
        .arg("-o")
        .arg(ToolValue::path(&job.output));
    inv.cwd = Some(job.work_dir.clone());
    inv.uploads.push(source_path);
    inv.downloads.push(job.output.clone());
    run(runner, &inv, "aarch64-linux-gnu-g++")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MockToolRunner, ToolOutput};

    fn job(root: &camino::Utf8Path, sdk: AiePsSdk) -> AiePsControlJob {
        AiePsControlJob::builder()
            .graph_name("tapa_aie".into())
            .sdk(sdk)
            .work_dir(root.join("work"))
            .output(root.join("vadd.aie_control.so"))
            .build()
    }

    #[test]
    fn source_is_design_specific_and_uses_the_ps_kernel_abi() {
        let text = source("tapa_aie_forever").expect("render");
        assert!(text.contains("kGraphName[] = \"tapa_aie_forever\""));
        for symbol in [
            "tapa_aie_control_init",
            "tapa_aie_control(",
            "tapa_aie_control_fini",
        ] {
            assert!(text.contains(symbol), "missing {symbol}");
        }
        source("other'; invalid").unwrap_err();
    }

    #[test]
    fn only_vck5000_requests_ps_control() {
        assert!(required("xilinx_vck5000_gen4x8_qdma_2_202220_1"));
        assert!(!required("xilinx_vck190_base_202520_1"));
        assert!(!required("xcvc1902-vsva2197-2MP-e-S"));
    }

    #[test]
    fn an_existing_sysroot_builds_one_shared_library() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = crate::util::utf8(dir.path());
        let job = job(&root, AiePsSdk::Sysroot("/common".into()));
        let runner = MockToolRunner::new();
        runner.push_ok("aarch64-linux-gnu-g++", ToolOutput::default());
        runner.attach_download(job.output.clone(), b"ps-control".to_vec());

        compile(&runner, &job).expect("compile");

        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        let args = calls[0].local_args();
        for expected in [
            "--sysroot=/common/sysroots/cortexa72-cortexa53-xilinx-linux",
            "-D__PS_ENABLE_AIE__",
            "-g",
            "-shared",
            "-lxrt_coreutil",
        ] {
            assert!(args.iter().any(|arg| arg == expected), "missing {expected}");
        }
        assert_eq!(fs_err::read(&job.output).expect("output"), b"ps-control");
    }

    #[test]
    fn the_firmware_sdk_is_extracted_before_cross_compiling() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = crate::util::utf8(dir.path());
        let job = job(&root, AiePsSdk::Installer("/firmware/sdk.sh".into()));
        let runner = MockToolRunner::new();
        runner.push_ok("sh", ToolOutput::default());
        runner.push_ok("aarch64-linux-gnu-g++", ToolOutput::default());
        runner.attach_download(job.output.clone(), b"ps-control".to_vec());

        compile(&runner, &job).expect("compile");

        let calls = runner.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0].local_args(),
            [
                "/firmware/sdk.sh",
                "-y",
                "-d",
                job.work_dir.join("sdk").as_str(),
            ]
        );
        assert!(calls[1].local_args().contains(&format!(
            "--sysroot={}",
            job.work_dir.join("sdk").join(SYSROOT)
        )));
    }
}
