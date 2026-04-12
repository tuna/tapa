use std::collections::HashMap;
use std::process::Command;

pub fn xilinx_environ() -> HashMap<String, String> {
    let tool = which::which("vitis_hls")
        .or_else(|_| which::which("vivado"))
        .ok();
    let mut env: HashMap<String, String> = std::env::vars().collect();

    if let Some(tool_path) = tool {
        if let Some(root) = tool_path.parent().and_then(|p| p.parent()) {
            let settings = root.join("settings64.sh");
            if settings.exists() {
                merge_sourced_env(&settings, &mut env);
            }
        }
    }

    if let Ok(xrt) = std::env::var("XILINX_XRT") {
        let setup = std::path::PathBuf::from(&xrt).join("setup.sh");
        if setup.exists() {
            merge_sourced_env(&setup, &mut env);
        }
    }

    env
}

fn merge_sourced_env(script: &std::path::Path, env: &mut HashMap<String, String>) {
    let out = Command::new("bash")
        .args(["-c", &format!(". {} && env -0", script.display())])
        .output();
    if let Ok(out) = out {
        if out.status.success() {
            for entry in String::from_utf8_lossy(&out.stdout).split('\0') {
                if let Some((k, v)) = entry.split_once('=') {
                    env.insert(k.to_owned(), v.to_owned());
                }
            }
        }
    }
}
