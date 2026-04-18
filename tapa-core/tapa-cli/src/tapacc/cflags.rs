//! Pure-function ports of `tapa.common.paths.{get_tapa_cflags,
//! get_tapacc_cflags, get_remote_hls_cflags, get_system_cflags}`.
//!
//! The vendor include resolution and macOS sysroot probe live behind
//! pluggable closures so unit tests can drive deterministic fixtures.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::Result;
use crate::tapacc::discover::find_resource;

/// Output of [`get_tapa_cflags`]. Mirrors the Python tuple, preserving
/// order so vendor headers cascade correctly through `tapacc`.
pub fn get_tapa_cflags() -> Vec<String> {
    let mut flags = Vec::<String>::new();

    if let Ok(tapa_lib_include) = find_resource("tapa-lib-include") {
        if tapa_lib_include.join("tapa.h").exists() {
            flags.push(format!("-isystem{}", tapa_lib_include.display()));
            for resource in ["fpga-runtime-include", "tapa-extra-runtime-include"] {
                if let Ok(extra) = find_resource(resource) {
                    if extra != tapa_lib_include {
                        flags.push(format!("-isystem{}", extra.display()));
                    }
                }
            }
        }
    }

    flags.extend(
        [
            "-Wno-attributes",
            "-Wno-unknown-pragmas",
            "-Wno-unused-label",
            "-D__builtin_FILE()=__FILE__",
            "-D__builtin_LINE()=__LINE__",
        ]
        .iter()
        .map(ToString::to_string),
    );

    flags
}

/// Compose the CFLAGS that `tapacc` itself wants. Mirrors
/// `get_tapacc_cflags(for_remote_hls)`.
pub fn get_tapacc_cflags(for_remote_hls: bool) -> Vec<String> {
    let include_gcc = is_linux() || for_remote_hls;
    let mut flags = Vec::<String>::new();

    let vendor_paths = vendor_include_paths(include_gcc);
    let mut vendor_flags: Vec<String> = vendor_paths
        .iter()
        .map(|p| format!("-isystem{}", p.display()))
        .collect();

    let nostdinc = !vendor_flags.is_empty() && include_gcc;
    if nostdinc {
        flags.push("-nostdinc++".to_string());
    }
    flags.extend(get_tapa_cflags());
    flags.append(&mut vendor_flags);

    if for_remote_hls && is_macos() {
        flags.push(
            "-D__assert_rtn(func,file,line,expr)=__assert_fail(expr,file,line,func)"
                .to_string(),
        );
    }

    flags
}

/// CFLAGS for remote-HLS C++ compilation.
pub fn get_remote_hls_cflags() -> Vec<String> {
    let mut flags = get_tapa_cflags();
    if is_macos() {
        flags.push(
            "-D__assert_rtn(func,file,line,expr)=__assert_fail(expr,file,line,func)"
                .to_string(),
        );
    }
    flags
}

/// CFLAGS for compiling C++ with the system clang/llvm. Mirrors
/// `get_system_cflags()`: `-isysroot` from `xcrun --show-sdk-path` on
/// macOS plus `-idirafter <tapa-system-include>` if available.
pub fn get_system_cflags() -> Vec<String> {
    let mut flags = macos_sysroot_flags();
    if let Ok(p) = find_resource("tapa-system-include") {
        flags.push(format!("-idirafter{}", p.display()));
    }
    flags
}

/// LDFLAGS for linking TAPA programs.
///
/// Mirrors `tapa/common/paths.py::get_tapa_ldflags`: derives `-L` and
/// `-Wl,-rpath` from `find_resource("fpga-runtime-lib")` /
/// `find_resource("tapa-lib-lib")`, plus every external library
/// directory the Bazel runfiles tree provides (gflags, glog,
/// tinyxml2, yaml-cpp, boost). Without the runfiles dirs, links from
/// the `bazel run //tapa:tapa -- g++` wrapper would fail to resolve
/// `-lgflags`, `-lglog`, etc.
pub fn get_tapa_ldflags() -> Vec<String> {
    use std::collections::BTreeSet;
    let mut libs: BTreeSet<PathBuf> = BTreeSet::new();
    if let Ok(p) = find_resource("fpga-runtime-lib") {
        libs.insert(p);
    }
    if let Ok(p) = find_resource("tapa-lib-lib") {
        libs.insert(p);
    }
    libs.extend(find_external_lib_in_runfiles());
    let mut out = Vec::<String>::new();
    for lib in &libs {
        out.push(format!("-Wl,-rpath,{}", lib.display()));
    }
    for lib in &libs {
        out.push(format!("-L{}", lib.display()));
    }
    for name in [
        "tapa", "frt_cpp", "context", "thread", "frt", "asio", "filesystem", "glog",
        "gflags", "OpenCL", "minizip_ng", "tinyxml2", "z", "yaml-cpp", "stdc++fs",
    ] {
        out.push(format!("-l{name}"));
    }
    out
}

/// Port of `tapa.common.paths.find_external_lib_in_runfiles`. Walks
/// the parents of the binary looking for a `tapa.runfiles` tree and
/// returns the external library directories Bazel stages there
/// (gflags, glog, tinyxml2, yaml-cpp, boost). Outside of Bazel, no
/// `tapa.runfiles` exists and the helper returns an empty vector.
fn find_external_lib_in_runfiles() -> Vec<PathBuf> {
    let anchor = std::env::var_os("TAPA_CLI_SEARCH_ANCHOR")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    let mut cursor: Option<&Path> = Some(anchor.as_path());
    while let Some(parent) = cursor {
        let candidate = parent.join("tapa.runfiles");
        if candidate.is_dir() {
            return [
                "gflags+",
                "glog+",
                "tinyxml2+",
                "yaml-cpp+",
                "rules_boost++non_module_dependencies+boost",
            ]
            .iter()
            .map(|leaf| candidate.join(leaf))
            .filter(|p| p.exists())
            .collect();
        }
        cursor = parent.parent();
    }
    Vec::new()
}

fn vendor_include_paths(include_gcc: bool) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut hls_root: Option<PathBuf> = None;
    for env_name in ["XILINX_HLS", "XILINX_VITIS"] {
        if let Some(path) = std::env::var_os(env_name) {
            let root = PathBuf::from(path);
            let include = root.join("include");
            if include.exists() {
                paths.push(include);
                hls_root = Some(root);
                break;
            }
        }
    }

    if include_gcc {
        if let Some(root) = hls_root {
            paths.extend(vendor_gcc_paths(&root));
        }
    }
    paths
}

fn vendor_gcc_paths(hls_root: &Path) -> Vec<PathBuf> {
    let tps = hls_root.join("tps").join("lnx64");
    let mut versions: Vec<(String, PathBuf)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(&tps) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let s = name.to_string_lossy();
        if let Some(rest) = s.strip_prefix("gcc-") {
            versions.push((rest.to_string(), entry.path()));
        }
    }
    versions.sort_by_key(|v| version_key(&v.0));
    let Some((latest, dir)) = versions.last() else {
        return Vec::new();
    };

    let cpp_include = dir.join("include").join("c++").join(latest);
    if !cpp_include.exists() {
        return Vec::new();
    }
    let mut out = vec![cpp_include.clone()];
    for arch in ["x86_64-pc-linux-gnu", "x86_64-linux-gnu"] {
        let p = cpp_include.join(arch);
        if p.exists() {
            out.push(p);
            break;
        }
    }
    out
}

fn version_key(s: &str) -> Vec<u32> {
    s.split('.').filter_map(|x| x.parse().ok()).collect()
}

fn macos_sysroot_flags() -> Vec<String> {
    if !is_macos() {
        return Vec::new();
    }
    let Ok(out) = Command::new("xcrun").args(["--show-sdk-path"]).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        Vec::new()
    } else {
        vec!["-isysroot".to_string(), path]
    }
}

fn is_linux() -> bool {
    cfg!(target_os = "linux")
}

fn is_macos() -> bool {
    cfg!(target_os = "macos")
}

/// Full `tapacc` argv (without the binary path itself). Mirrors the
/// argument tuple in `tapa/steps/analyze.py::run_tapacc`.
pub fn run_tapacc_argv(
    files: &[PathBuf],
    top: &str,
    cflags: &[String],
    target: &str,
) -> Vec<String> {
    let mut argv = Vec::<String>::new();
    for f in files {
        argv.push(f.display().to_string());
    }
    argv.push("-top".to_string());
    argv.push(top.to_string());
    argv.push("--target".to_string());
    argv.push(target.to_string());
    argv.push("--".to_string());
    argv.extend(cflags.iter().cloned());
    argv.push("-DTAPA_TARGET_DEVICE_".to_string());
    argv.push("-DTAPA_TARGET_STUB_".to_string());
    argv
}

/// Convenience for callers that don't need typed CFLAGS.
pub fn _unused() -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_tapacc_argv_matches_python_shape() {
        let argv = run_tapacc_argv(
            &[PathBuf::from("a.cpp"), PathBuf::from("b.cpp")],
            "Top",
            &["-DFOO=1".to_string(), "-Iinc".to_string()],
            "xilinx-hls",
        );
        assert_eq!(
            argv,
            vec![
                "a.cpp",
                "b.cpp",
                "-top",
                "Top",
                "--target",
                "xilinx-hls",
                "--",
                "-DFOO=1",
                "-Iinc",
                "-DTAPA_TARGET_DEVICE_",
                "-DTAPA_TARGET_STUB_",
            ],
        );
    }

    #[test]
    fn version_key_sorts_numerically() {
        let mut versions = vec!["10.2.0", "9.5.0", "11.0.1"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        versions.sort_by_key(|v| version_key(v));
        assert_eq!(versions, vec!["9.5.0", "10.2.0", "11.0.1"]);
    }
}
