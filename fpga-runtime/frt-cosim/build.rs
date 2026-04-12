use std::fs;
use std::path::Path;

fn main() {
    emit_rerun_if_changed(Path::new("templates"));
}

fn emit_rerun_if_changed(dir: &Path) {
    println!("cargo:rerun-if-changed={}", dir.display());

    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            emit_rerun_if_changed(&path);
        } else {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}
