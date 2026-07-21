pub mod hls;
pub mod package_xo;
pub mod vitis;
pub mod vivado;

fn merged_failure_output(stdout: String, stderr: String) -> String {
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, _) => stderr,
        (_, true) => stdout,
        (false, false) => format!("stdout:\n{stdout}\nstderr:\n{stderr}"),
    }
}
