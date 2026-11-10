//! The `v++` link of a design whose top runs on the AI Engine, with no PL
//! kernel: `--link` on `libadf.a` alone, then `--package` for SD boot with
//! the AI Engine cores left for the host to start, then TAPA's argument list
//! added to the xclbin as `USER_METADATA`, where the runtime reads it. The
//! `.xsa` stays beside the xclbin: packaging an SD image reads it.

use camino::{Utf8Path, Utf8PathBuf};
use typed_builder::TypedBuilder;

use crate::error::{Result, XilinxError};
use crate::runtime::process::{ToolInvocation, ToolRunner};
use crate::tools::{tool_failure, validate_input, LinkOutput};
use crate::ToolValue;

#[derive(Debug, Clone, TypedBuilder)]
pub struct AieLinkJob {
    pub platform: String,
    /// Hardware emulation (`hw_emu`) instead of hardware.
    pub emulation: bool,
    pub libadf: Utf8PathBuf,
    /// The JSON the runtime reads from the xclbin.
    pub arguments: Utf8PathBuf,
    /// The tool working directory, which keeps the intermediate files.
    pub work_dir: Utf8PathBuf,
    pub output_xclbin: Utf8PathBuf,
    #[builder(default = "v++".to_string())]
    pub vpp: String,
    #[builder(default = "xclbinutil".to_string())]
    pub xclbinutil: String,
}

/// The `.xsa` a link leaves beside `xclbin`.
#[must_use]
pub fn xsa(xclbin: &Utf8Path) -> Utf8PathBuf {
    xclbin.with_extension("xsa")
}

/// Links, packages and annotates the graph, returning the xclbin.
pub fn link(runner: &dyn ToolRunner, job: &AieLinkJob) -> Result<LinkOutput> {
    // The tools run in the work directory, where a relative path would land.
    let job = &AieLinkJob {
        libadf: crate::util::absolutize(&job.libadf),
        arguments: crate::util::absolutize(&job.arguments),
        work_dir: crate::util::absolutize(&job.work_dir),
        output_xclbin: crate::util::absolutize(&job.output_xclbin),
        ..job.clone()
    };
    validate_input(&job.libadf, "AI Engine graph", XilinxError::VitisLink)?;
    validate_input(&job.arguments, "argument list", XilinxError::VitisLink)?;
    fs_err::create_dir_all(&job.work_dir)?;
    if let Some(parent) = job.output_xclbin.parent() {
        fs_err::create_dir_all(parent)?;
    }
    let xsa = xsa(&job.output_xclbin);
    let packaged = job.work_dir.join("packaged.xclbin");
    for stale in [&job.output_xclbin, &xsa, &packaged] {
        if stale.exists() {
            fs_err::remove_file(stale)?;
        }
    }
    let target = if job.emulation { "hw_emu" } else { "hw" };
    let vpp = |step: &str| {
        ToolInvocation::new(job.vpp.clone())
            .arg(step)
            .arg("--target")
            .arg(target)
            .arg("--platform")
            .arg(job.platform.clone())
            .arg("--save-temps")
    };
    let link = vpp("--link")
        .arg("--temp_dir")
        .arg(ToolValue::path(job.work_dir.join("link.tmp")))
        .arg("--output")
        .arg(ToolValue::path(&xsa))
        .arg(ToolValue::path(&job.libadf));
    let package = vpp("--package")
        .arg("--temp_dir")
        .arg(ToolValue::path(job.work_dir.join("package.tmp")))
        .arg("--package.boot_mode")
        .arg("sd")
        .arg("--package.defer_aie_run")
        .arg("--package.no_image")
        .arg("--package.out_dir")
        .arg(ToolValue::path(job.work_dir.join("package")))
        .arg("--output")
        .arg(ToolValue::path(&packaged))
        .arg(ToolValue::path(&xsa))
        .arg(ToolValue::path(&job.libadf));
    let annotate = ToolInvocation::new(job.xclbinutil.clone())
        .arg("--input")
        .arg(ToolValue::path(&packaged))
        .arg("--add-section")
        .arg(ToolValue::prefixed_path(
            "USER_METADATA:RAW:",
            &job.arguments,
        ))
        .arg("--output")
        .arg(ToolValue::path(&job.output_xclbin))
        .arg("--force");
    // What each step reads and writes, for a remote tool host.
    let steps = [
        (link, &job.vpp, vec![job.libadf.clone()], vec![xsa.clone()]),
        (
            package,
            &job.vpp,
            vec![xsa, job.libadf.clone()],
            vec![packaged.clone()],
        ),
        (
            annotate,
            &job.xclbinutil,
            vec![packaged, job.arguments.clone()],
            vec![job.output_xclbin.clone()],
        ),
    ];
    let (mut stdout, mut stderr) = (String::new(), String::new());
    for (mut inv, program, uploads, downloads) in steps {
        inv.cwd = Some(job.work_dir.clone());
        inv.env
            .insert("HOME".into(), ToolValue::path(&job.work_dir));
        inv.uploads = uploads;
        inv.downloads = downloads;
        let out = runner.run(&inv)?;
        if out.exit_code != 0 {
            return Err(tool_failure(program, out));
        }
        stdout.push_str(&out.stdout);
        stderr.push_str(&out.stderr);
    }
    validate_input(&job.output_xclbin, "xclbin", XilinxError::VitisLink)?;
    Ok(LinkOutput {
        bitstream: job.output_xclbin.clone(),
        requested_mhz: 0,
        recorded_mhz: 0.0,
        timing: None,
        timing_report: None,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MockToolRunner;

    #[test]
    fn link_hands_v_plus_plus_absolute_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = crate::util::utf8(dir.path());
        fs_err::write(root.join("libadf.a"), "graph").expect("graph");
        fs_err::write(root.join("arguments.json"), "[]").expect("arguments");
        let job = AieLinkJob::builder()
            .platform("xilinx_vck190_base_202520_1".into())
            .emulation(false)
            .libadf(root.join("libadf.a"))
            .arguments(root.join("arguments.json"))
            .work_dir(root.join("work"))
            .output_xclbin("aie-link-test.xclbin".into())
            .build();
        let runner = MockToolRunner::new();
        runner.push_failure("v++", 1, "stop after the first step");
        link(&runner, &job).expect_err("the link step fails");
        let args = runner.calls()[0].local_args();
        let output = args.iter().skip_while(|arg| *arg != "--output").nth(1);
        let output = output.expect("--output");
        assert!(Utf8Path::new(output).is_absolute(), "{output}");
    }
}
