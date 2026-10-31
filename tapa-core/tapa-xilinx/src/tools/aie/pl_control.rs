use camino::Utf8PathBuf;
use typed_builder::TypedBuilder;

use crate::error::Result;
use crate::runtime::process::{ToolInvocation, ToolRunner};
use crate::tools::tool_failure;
use crate::ToolValue;

const SOURCE: &str = include_str!("pl_control.cpp");
pub const KERNEL_NAME: &str = "tapa_aie_pl_control";
const MAX_PROGRAM_WORDS: usize = 1024;

#[must_use]
pub fn required(platform: &str) -> bool {
    platform.contains("vck5000")
}

#[derive(Debug, Clone, TypedBuilder)]
pub struct AiePlControlJob {
    pub platform: String,
    pub target_mhz: u32,
    pub work_dir: Utf8PathBuf,
    pub output: Utf8PathBuf,
}

pub fn compile(runner: &dyn ToolRunner, job: &AiePlControlJob) -> Result<()> {
    fs_err::create_dir_all(&job.work_dir)?;
    if let Some(parent) = job.output.parent() {
        fs_err::create_dir_all(parent)?;
    }
    let source = job.work_dir.join("pl_control.cpp");
    fs_err::write(&source, SOURCE)?;
    let mut inv = ToolInvocation::new("v++")
        .arg("--compile")
        .arg("--target")
        .arg("hw")
        .arg("--platform")
        .arg(job.platform.clone())
        .arg("--clock.defaultFreqHz")
        .arg((u64::from(job.target_mhz) * 1_000_000).to_string())
        .arg("--kernel")
        .arg(KERNEL_NAME)
        .arg(ToolValue::path(&source))
        .arg("--output")
        .arg(ToolValue::path(&job.output));
    inv.cwd = Some(job.work_dir.clone());
    inv.env
        .insert("HOME".into(), ToolValue::path(&job.work_dir));
    inv.uploads.push(source);
    inv.downloads.push(job.output.clone());
    let out = runner.run(&inv)?;
    if out.exit_code != 0 {
        return Err(tool_failure("v++", out));
    }
    Ok(())
}

pub fn encode(writes: &[(u32, u32)]) -> Option<Vec<u8>> {
    let words = writes.len().checked_mul(2)?;
    if words > MAX_PROGRAM_WORDS {
        return None;
    }
    let mut out = Vec::with_capacity((words + 1) * 4);
    out.extend_from_slice(&(words as u32).to_le_bytes());
    for (address, value) in writes {
        out.extend_from_slice(&address.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MockToolRunner, ToolOutput};

    #[test]
    fn only_vck5000_requires_pl_control() {
        assert!(required("xilinx_vck5000_gen4x8_qdma_2_202220_1"));
        assert!(!required("xilinx_vck190_base_202520_1"));
        assert!(!required("xcvc1902-vsva2197-2MP-e-S"));
    }

    #[test]
    fn compile_builds_the_control_xo_at_the_design_clock() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = crate::util::utf8(dir.path());
        let job = AiePlControlJob::builder()
            .platform("xilinx_vck5000_gen4x8_qdma_2_202220_1".into())
            .target_mhz(300)
            .work_dir(root.join("work"))
            .output(root.join("vadd.aie_control.xo"))
            .build();
        let runner = MockToolRunner::new();
        runner.push_ok("v++", ToolOutput::default());
        runner.attach_download(job.output.clone(), b"controller".to_vec());
        compile(&runner, &job).expect("compile");
        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        let args = calls[0].local_args();
        for expected in [
            "--compile",
            "--clock.defaultFreqHz",
            "300000000",
            "--kernel",
            KERNEL_NAME,
        ] {
            assert!(args.iter().any(|arg| arg == expected), "missing {expected}");
        }
        assert_eq!(fs_err::read(job.output).expect("output"), b"controller");
    }

    #[test]
    fn programs_carry_a_word_count_and_register_pairs() {
        assert_eq!(
            encode(&[(0x2004, 1), (0x1fe0, 64)]).expect("encode"),
            [4, 0, 0, 0, 4, 0x20, 0, 0, 1, 0, 0, 0, 0xe0, 0x1f, 0, 0, 64, 0, 0, 0,]
        );
        assert!(encode(&vec![(0, 0); MAX_PROGRAM_WORDS / 2 + 1]).is_none());
    }
}
