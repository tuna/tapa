//! Xilinx tool discovery and CFLAGS construction.
//!
//! Ports `tapa/common/paths.py::get_xilinx_tool_path`,
//! `_get_vendor_include_paths`, `get_tapa_cflags`, `get_tapacc_cflags`,
//! and `get_remote_hls_cflags`. The flag ordering mirrors the Python
//! tuple returns exactly so parity tests can compare vectors
//! element-by-element.

use std::path::{Path, PathBuf};

use crate::error::{Result, XilinxError};

/// Which Xilinx tool root to resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XilinxToolPath {
    Hls,
    Vitis,
}

impl XilinxToolPath {
    const fn env_var(self) -> &'static str {
        match self {
            Self::Hls => "XILINX_HLS",
            Self::Vitis => "XILINX_VITIS",
        }
    }
}

fn resolve_tool(kind: XilinxToolPath) -> Result<PathBuf> {
    let var = kind.env_var();
    let raw = std::env::var(var).map_err(|_| match kind {
        XilinxToolPath::Hls => XilinxError::MissingXilinxHls,
        XilinxToolPath::Vitis => XilinxError::ToolNotFound(var.into()),
    })?;
    let path = PathBuf::from(raw);
    if !path.exists() {
        return Err(XilinxError::ToolNotFound(path.display().to_string()));
    }
    Ok(path)
}

pub fn get_xilinx_hls() -> Result<PathBuf> {
    resolve_tool(XilinxToolPath::Hls)
}

pub fn get_xilinx_vitis() -> Result<PathBuf> {
    resolve_tool(XilinxToolPath::Vitis)
}

fn pick_hls_include_root() -> Option<PathBuf> {
    for kind in [XilinxToolPath::Hls, XilinxToolPath::Vitis] {
        if let Ok(root) = resolve_tool(kind) {
            let inc = root.join("include");
            if inc.exists() {
                return Some(inc);
            }
        }
    }
    None
}

fn latest_vendor_gcc(hls_root: &Path) -> Option<(String, PathBuf)> {
    let tps_lnx64 = hls_root.join("tps").join("lnx64");
    let entries = std::fs::read_dir(&tps_lnx64).ok()?;
    let mut versions: Vec<(Vec<u32>, String, PathBuf)> = Vec::new();
    for ent in entries.flatten() {
        let name = ent.file_name().to_string_lossy().into_owned();
        if let Some(rest) = name.strip_prefix("gcc-") {
            let parts: std::result::Result<Vec<u32>, _> =
                rest.split('.').map(str::parse).collect();
            if let Ok(parts) = parts {
                versions.push((parts, rest.to_string(), ent.path()));
            }
        }
    }
    versions.sort_by(|a, b| a.0.cmp(&b.0));
    let (_, ver, path) = versions.into_iter().last()?;
    Some((ver, path))
}

fn vendor_include_paths_inner(include_gcc: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Some(inc) = pick_hls_include_root() else {
        return out;
    };
    out.push(inc.clone());

    if !include_gcc {
        return out;
    }

    let Some(hls_root) = inc.parent().map(Path::to_path_buf) else {
        return out;
    };
    let Some((gcc_ver, gcc_path)) = latest_vendor_gcc(&hls_root) else {
        return out;
    };
    let cpp_include = gcc_path.join("include").join("c++").join(&gcc_ver);
    if !cpp_include.exists() {
        return out;
    }
    out.push(cpp_include.clone());
    for arch in ["x86_64-pc-linux-gnu", "x86_64-linux-gnu"] {
        let p = cpp_include.join(arch);
        if p.exists() {
            out.push(p);
            break;
        }
    }
    out
}

/// Vendor include paths for local compilation.
///
/// GCC C++ headers are Linux-only (they require glibc); non-Linux hosts
/// get only the HLS `include/` directory.
pub fn get_vendor_include_paths() -> Vec<PathBuf> {
    vendor_include_paths_inner(cfg!(target_os = "linux"))
}

/// TAPA runtime include directories in the order Python prepends them.
///
/// Mirrors `tapa/common/paths.py::get_tapa_cflags`: `tapa-lib-include`
/// first (required to make Vitis happy), then the optional
/// `fpga-runtime-include` and `tapa-extra-runtime-include` when they
/// exist. Each slot is resolved in order from:
/// 1. An explicit env override — e.g. `TAPA_LIB_INCLUDE` for the
///    primary include directory.
/// 2. `TAPA_INCLUDE_DIRS` — a `:`-separated list, first entry used.
///
/// When nothing resolves, the slot is skipped (matches Python's
/// "warn and continue" semantics when TAPA runtime libs are not
/// installed).
fn resolve_tapa_include(env_key: &str) -> Option<PathBuf> {
    if let Ok(raw) = std::env::var(env_key) {
        let p = PathBuf::from(raw);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Candidate subpaths for each logical include slot. Mirrors
/// `POTENTIAL_PATHS` in `tapa/common/paths.py`.
const TAPA_LIB_INCLUDE_SUBPATHS: &[&str] = &["tapa-lib", "usr/include"];
const FPGA_RUNTIME_INCLUDE_SUBPATHS: &[&str] = &["fpga-runtime", "usr/include"];
const TAPA_EXTRA_RUNTIME_INCLUDE_SUBPATHS: &[&str] = &[
    "tapa-system-include/tapa-extra-runtime-include",
    "tapa-lib/extra-runtime-include",
    "usr/include",
];

#[cfg(debug_assertions)]
pub fn debug_search_roots() -> Vec<PathBuf> {
    search_roots()
}

fn search_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    // Walk parents of the loaded-extension location when known
    // (matches Python's `Path(__file__).absolute().parents`). The
    // PyO3 wrapper exports `TAPA_XILINX_BINDINGS_DIR` pointing at the
    // directory holding the `tapa_core` extension, so installed
    // packages can resolve their sibling `tapa-lib/` include dir.
    if let Ok(dir) = std::env::var("TAPA_XILINX_BINDINGS_DIR") {
        let mut p = PathBuf::from(dir);
        loop {
            roots.push(p.clone());
            match p.parent() {
                Some(next) => p = next.to_path_buf(),
                None => break,
            }
        }
    }
    // Walk parents of the running executable as a fallback.
    if let Ok(exe) = std::env::current_exe() {
        let mut p = exe.as_path();
        while let Some(parent) = p.parent() {
            roots.push(parent.to_path_buf());
            p = parent;
        }
    }
    // Also walk parents of the current working directory as a
    // fallback for repo-checkout runs invoking cargo from tapa-core/.
    if let Ok(cwd) = std::env::current_dir() {
        let mut p = cwd.as_path();
        loop {
            roots.push(p.to_path_buf());
            match p.parent() {
                Some(next) => p = next,
                None => break,
            }
        }
    }
    roots
}

fn find_resource(subpaths: &[&str], sentinel: Option<&str>) -> Option<PathBuf> {
    for root in search_roots() {
        for sub in subpaths {
            let candidate = root.join(sub);
            if !candidate.exists() {
                continue;
            }
            if let Some(s) = sentinel {
                if !candidate.join(s).exists() {
                    continue;
                }
            }
            return Some(candidate);
        }
    }
    None
}

fn tapa_include_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let autodiscover = std::env::var("TAPA_DISABLE_INCLUDE_AUTODISCOVERY").is_err();
    let auto = |subs, sentinel| {
        if autodiscover {
            find_resource(subs, sentinel)
        } else {
            None
        }
    };

    // tapa-lib-include: env override first, then auto-discovery
    // requiring `tapa.h` as a sentinel (matches Python's extra
    // validation step).
    let tapa_lib = resolve_tapa_include("TAPA_LIB_INCLUDE")
        .or_else(|| auto(TAPA_LIB_INCLUDE_SUBPATHS, Some("tapa.h")));
    if let Some(p) = tapa_lib {
        if seen.insert(p.clone()) {
            out.push(p);
        }
    }

    // fpga-runtime-include, tapa-extra-runtime-include: optional.
    let fpga = resolve_tapa_include("TAPA_FPGA_RUNTIME_INCLUDE")
        .or_else(|| auto(FPGA_RUNTIME_INCLUDE_SUBPATHS, None));
    if let Some(p) = fpga {
        if seen.insert(p.clone()) {
            out.push(p);
        }
    }

    let extra = resolve_tapa_include("TAPA_EXTRA_RUNTIME_INCLUDE")
        .or_else(|| auto(TAPA_EXTRA_RUNTIME_INCLUDE_SUBPATHS, None));
    if let Some(p) = extra {
        if seen.insert(p.clone()) {
            out.push(p);
        }
    }

    if let Ok(dirs) = std::env::var("TAPA_INCLUDE_DIRS") {
        for part in dirs.split(':').filter(|s| !s.is_empty()) {
            let p = PathBuf::from(part);
            if p.exists() && seen.insert(p.clone()) {
                out.push(p);
            }
        }
    }
    out
}

/// Base TAPA CFLAGS: TAPA runtime includes + warning suppressions +
/// builtin-define shims.
///
/// Mirrors `tapa/common/paths.py::get_tapa_cflags`, including the
/// leading `-isystem` entries for TAPA runtime include directories.
pub fn get_tapa_cflags() -> Vec<String> {
    let mut out = Vec::new();
    for p in tapa_include_dirs() {
        out.push(format!("-isystem{}", p.display()));
    }
    out.extend([
        "-Wno-attributes".into(),
        "-Wno-unknown-pragmas".into(),
        "-Wno-unused-label".into(),
        "-D__builtin_FILE()=__FILE__".into(),
        "-D__builtin_LINE()=__LINE__".into(),
    ]);
    out
}

fn darwin_assert_compat() -> String {
    "-D__assert_rtn(func,file,line,expr)=__assert_fail(expr,file,line,func)".into()
}

/// CFLAGS for `tapacc` with HLS vendor libraries.
///
/// Mirrors `tapa/common/paths.py::get_tapacc_cflags(for_remote_hls)`.
/// When `for_remote_hls` is true, GCC vendor stdlib headers are
/// included regardless of the local OS; this matches running HLS on a
/// remote Linux host from a macOS workstation.
pub fn get_tapacc_cflags(for_remote_hls: bool) -> Vec<String> {
    let include_gcc = cfg!(target_os = "linux") || for_remote_hls;
    let vendor = vendor_include_paths_inner(include_gcc);

    let mut vendor_flags: Vec<String> = Vec::new();
    for p in &vendor {
        vendor_flags.push(format!("-isystem{}", p.display()));
    }

    let mut out = Vec::new();
    if !vendor_flags.is_empty() && include_gcc {
        out.push("-nostdinc++".into());
    }
    out.extend(get_tapa_cflags());
    out.extend(vendor_flags);
    if for_remote_hls && cfg!(target_os = "macos") {
        out.push(darwin_assert_compat());
    }
    out
}

/// CFLAGS for remote HLS compilation from the current host.
///
/// Mirrors `tapa/common/paths.py::get_remote_hls_cflags`: base TAPA
/// CFLAGS plus the Darwin `__assert_rtn` compatibility define when
/// running on macOS.
pub fn get_remote_hls_cflags() -> Vec<String> {
    let mut out = get_tapa_cflags();
    if cfg!(target_os = "macos") {
        out.push(darwin_assert_compat());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &Path) -> Self {
            let prev = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, prev }
        }

        fn unset(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::remove_var(key);
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn missing_xilinx_hls_returns_typed_error() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::unset("XILINX_HLS");
        let _g2 = EnvGuard::unset("XILINX_VITIS");
        assert!(matches!(get_xilinx_hls(), Err(XilinxError::MissingXilinxHls)));
    }

    #[test]
    fn nonexistent_xilinx_hls_returns_tool_not_found() {
        let _lock = ENV_LOCK.lock().unwrap();
        let bogus = PathBuf::from("/definitely/not/a/real/xilinx/install");
        let _g = EnvGuard::set("XILINX_HLS", &bogus);
        assert!(matches!(
            get_xilinx_hls(),
            Err(XilinxError::ToolNotFound(_))
        ));
    }

    #[test]
    fn tapa_cflags_shape_matches_python_when_include_unset() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g1 = EnvGuard::unset("TAPA_LIB_INCLUDE");
        let _g2 = EnvGuard::unset("TAPA_FPGA_RUNTIME_INCLUDE");
        let _g3 = EnvGuard::unset("TAPA_EXTRA_RUNTIME_INCLUDE");
        let _g4 = EnvGuard::unset("TAPA_INCLUDE_DIRS");
        let bogus = PathBuf::from("/tmp/tapa-xilinx-no-autodiscovery");
        let _g5 = EnvGuard::set("TAPA_DISABLE_INCLUDE_AUTODISCOVERY", &bogus);
        let expected = vec![
            "-Wno-attributes",
            "-Wno-unknown-pragmas",
            "-Wno-unused-label",
            "-D__builtin_FILE()=__FILE__",
            "-D__builtin_LINE()=__LINE__",
        ];
        let flags = get_tapa_cflags();
        let got: Vec<&str> = flags.iter().map(String::as_str).collect();
        assert_eq!(got, expected);
    }

    #[test]
    fn tapa_cflags_prepends_include_dirs_when_resolved() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let lib = tmp.path().join("tapa-lib");
        std::fs::create_dir_all(&lib).unwrap();
        let _g = EnvGuard::set("TAPA_LIB_INCLUDE", &lib);
        let _g2 = EnvGuard::unset("TAPA_FPGA_RUNTIME_INCLUDE");
        let _g3 = EnvGuard::unset("TAPA_EXTRA_RUNTIME_INCLUDE");
        let _g4 = EnvGuard::unset("TAPA_INCLUDE_DIRS");
        let bogus = PathBuf::from("/tmp/tapa-xilinx-no-autodiscovery");
        let _g5 = EnvGuard::set("TAPA_DISABLE_INCLUDE_AUTODISCOVERY", &bogus);
        let flags = get_tapa_cflags();
        assert!(flags[0].starts_with("-isystem"));
        assert!(flags[0].contains("tapa-lib"));
        assert_eq!(flags[1], "-Wno-attributes");
    }

    #[test]
    fn remote_hls_cflags_include_darwin_compat_only_on_mac() {
        let flags = get_remote_hls_cflags();
        let has_darwin = flags.iter().any(|f| f.starts_with("-D__assert_rtn"));
        assert_eq!(has_darwin, cfg!(target_os = "macos"));
    }

    #[test]
    fn tapacc_cflags_adds_nostdincpp_when_gcc_vendor_present() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("include")).unwrap();
        let gcc = root.join("tps/lnx64/gcc-11.4.0/include/c++/11.4.0/x86_64-pc-linux-gnu");
        std::fs::create_dir_all(&gcc).unwrap();
        let _g = EnvGuard::set("XILINX_HLS", root);
        let _g2 = EnvGuard::unset("XILINX_VITIS");
        let bogus = PathBuf::from("/tmp/tapa-xilinx-no-autodiscovery");
        let _g3 = EnvGuard::set("TAPA_DISABLE_INCLUDE_AUTODISCOVERY", &bogus);
        let _g4 = EnvGuard::unset("TAPA_LIB_INCLUDE");
        let _g5 = EnvGuard::unset("TAPA_FPGA_RUNTIME_INCLUDE");
        let _g6 = EnvGuard::unset("TAPA_EXTRA_RUNTIME_INCLUDE");
        let _g7 = EnvGuard::unset("TAPA_INCLUDE_DIRS");

        let flags = get_tapacc_cflags(true);
        assert_eq!(flags.first().map(String::as_str), Some("-nostdinc++"));
        // Warning flags must appear before vendor -isystem entries.
        let nostdincpp_idx = flags.iter().position(|f| f == "-nostdinc++").unwrap();
        let attr_idx = flags.iter().position(|f| f == "-Wno-attributes").unwrap();
        let first_isystem = flags
            .iter()
            .position(|f| f.starts_with("-isystem"))
            .unwrap();
        assert!(nostdincpp_idx < attr_idx);
        assert!(attr_idx < first_isystem);
    }

    #[test]
    fn tapacc_cflags_no_nostdincpp_without_vendor() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::unset("XILINX_HLS");
        let _g2 = EnvGuard::unset("XILINX_VITIS");
        let bogus = PathBuf::from("/tmp/tapa-xilinx-no-autodiscovery");
        let _g3 = EnvGuard::set("TAPA_DISABLE_INCLUDE_AUTODISCOVERY", &bogus);
        let _g4 = EnvGuard::unset("TAPA_LIB_INCLUDE");
        let _g5 = EnvGuard::unset("TAPA_FPGA_RUNTIME_INCLUDE");
        let _g6 = EnvGuard::unset("TAPA_EXTRA_RUNTIME_INCLUDE");
        let _g7 = EnvGuard::unset("TAPA_INCLUDE_DIRS");
        let flags = get_tapacc_cflags(true);
        assert!(flags.iter().all(|f| f != "-nostdinc++"));
    }

    #[test]
    fn resolves_existing_hls_root() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("include")).unwrap();
        let _g = EnvGuard::set("XILINX_HLS", tmp.path());
        let _g2 = EnvGuard::unset("XILINX_VITIS");
        assert_eq!(get_xilinx_hls().unwrap(), tmp.path());
    }
}
