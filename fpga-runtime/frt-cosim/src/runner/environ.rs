use std::collections::HashMap;
use std::process::Command;
use std::sync::OnceLock;

pub fn xilinx_environ() -> &'static HashMap<String, String> {
    static CACHE: OnceLock<HashMap<String, String>> = OnceLock::new();
    CACHE.get_or_init(compute_xilinx_environ)
}

fn compute_xilinx_environ() -> HashMap<String, String> {
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
    // The path goes in as an argument, not into the command text, where a
    // space or a `$` in an install path would change what is sourced; the
    // script then sees no arguments, as when sourced by hand.
    let out = Command::new("bash")
        .args(["-c", "script=$1; shift; . \"$script\" && env -0", "bash"])
        .arg(script)
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

#[cfg(test)]
mod tests {
    use super::merge_sourced_env;
    use std::collections::HashMap;

    /// Tool installs may sit under a path with spaces or shell
    /// metacharacters; the script is sourced from that path as is.
    #[test]
    fn a_settings_script_under_any_path_is_sourced() {
        let root = tempfile::tempdir().expect("temp dir");
        let dir = root.path().join("Xilinx Tools $HOME;");
        std::fs::create_dir_all(&dir).expect("install dir");
        let script = dir.join("settings64.sh");
        std::fs::write(&script, "export FRT_SOURCED_PROBE=1\n").expect("write script");
        let mut env = HashMap::new();
        merge_sourced_env(&script, &mut env);
        assert_eq!(env.get("FRT_SOURCED_PROBE").map(String::as_str), Some("1"));
    }
}
