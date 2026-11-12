//! Commands that compile nothing must not create the work directory: a
//! `tapa version` or `tapa g++` run in a source tree would otherwise leave a
//! `./work.out` behind.

use std::process::Command;

fn tapa_in(cwd: &std::path::Path, args: &[&str]) -> std::process::ExitStatus {
    Command::new(env!("CARGO_BIN_EXE_tapa"))
        .args(args)
        .current_dir(cwd)
        .env("TAPA_NO_UPDATE_CHECK", "1")
        .status()
        .expect("run tapa")
}

#[test]
fn commands_that_compile_nothing_leave_no_work_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(tapa_in(dir.path(), &["version"]).success());
    assert!(tapa_in(dir.path(), &["g++", "--executable", "true", "--", "-v"]).success());
    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .expect("read cwd")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert!(entries.is_empty(), "left behind: {entries:?}");
}
