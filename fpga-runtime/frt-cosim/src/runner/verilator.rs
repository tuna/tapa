use super::{configure_sim_command, environ::xilinx_environ, SimRunner};
use crate::context::CosimContext;
use crate::error::{CosimError, Result};
use crate::metadata::KernelSpec;
use crate::tb::verilator::VerilatorTbGenerator;
use regex_lite::Regex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use which::which;

const VERILATOR_BUILD_LOCK_ENV: &str = frt_shm::env::FRT_VERILATOR_BUILD_LOCK;

pub struct VerilatorRunner {
    pub dpi_lib: PathBuf,
}

fn resolve_verilator_bin(
    verilator_bin: Option<PathBuf>,
    runfiles_root: Option<PathBuf>,
) -> Result<PathBuf> {
    if let Some(bin) = verilator_bin {
        if bin.is_absolute() {
            return Ok(bin);
        }
        if let Some(root) = runfiles_root {
            let candidate = root.join(&bin);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
        let candidate = std::env::current_dir()?.join(&bin);
        if candidate.exists() {
            return Ok(candidate);
        }
        return Ok(bin);
    }
    which("verilator").map_err(|_e| CosimError::ToolNotFound("verilator".into()))
}

fn verilator_root_env(verilator_bin: &Path) -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("VERILATOR_ROOT") {
        return Some(PathBuf::from(root));
    }
    // Check candidates that have $ROOT/include/verilated.h — the layout
    // Verilator expects when VERILATOR_ROOT is set.
    [
        // Standard layout: bin is at $ROOT/bin/verilator
        verilator_bin
            .parent()
            .and_then(|p| p.parent())
            .map(PathBuf::from),
        // Bazel runfiles: bin.runfiles/verilator+/
        verilator_bin.parent().map(|p| {
            p.join(format!(
                "{}.runfiles/verilator+",
                verilator_bin
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            ))
        }),
    ]
    .into_iter()
    .flatten()
    .find(|c| c.join("include/verilated.h").exists())
}

impl SimRunner for VerilatorRunner {
    fn prepare(
        &self,
        spec: &KernelSpec,
        ctx: &CosimContext,
        scalar_values: &HashMap<u32, Vec<u8>>,
        tb_dir: &Path,
    ) -> Result<()> {
        let buffer_sizes: HashMap<String, usize> = ctx
            .buffers
            .iter()
            .map(|(name, seg)| (name.clone(), seg.len()))
            .collect();
        let generator =
            VerilatorTbGenerator::new(spec, &ctx.base_addresses, &buffer_sizes, scalar_values);
        std::fs::write(tb_dir.join("tb.cpp"), generator.render_tb()?)?;
        let rtl_dir = tb_dir.join("rtl");
        std::fs::create_dir_all(&rtl_dir)?;
        for f in spec.verilog_files.iter().chain(spec.tcl_files.iter()) {
            if let Some(fname) = f.file_name() {
                std::fs::copy(f, rtl_dir.join(fname))?;
            }
        }
        generate_xilinx_fp_ip_models(&rtl_dir)?;
        fix_combinational_nba(&rtl_dir)?;
        Ok(())
    }

    fn spawn(
        &self,
        spec: &KernelSpec,
        ctx: &CosimContext,
        tb_dir: &Path,
    ) -> Result<std::process::Child> {
        let runfiles_root = std::env::var_os("RUNFILES_DIR")
            .or_else(|| std::env::var_os("TEST_SRCDIR"))
            .map(PathBuf::from);
        let verilator_bin = resolve_verilator_bin(
            std::env::var_os("VERILATOR_BIN").map(PathBuf::from),
            runfiles_root,
        )?;
        let _build_gate = VerilatorBuildGate::acquire()?;

        let top = &spec.top_name;
        let rtl_dir = tb_dir.join("rtl");
        let mut args = vec![
            "--cc".to_owned(),
            "--top-module".to_owned(),
            top.clone(),
            "--no-timing".to_owned(),
            // HLS-generated sequential-init loops can explode during Verilation.
            // Disabling automatic procedural unrolling keeps compile memory bounded
            // without changing RTL semantics.
            "--unroll-count".to_owned(),
            "0".to_owned(),
            "--exe".to_owned(),
            "tb.cpp".to_owned(),
            "-LDFLAGS".to_owned(),
            self.dpi_lib.to_string_lossy().to_string(),
            "-Wno-fatal".to_owned(),
            "-Wno-PINMISSING".to_owned(),
            "-Wno-WIDTH".to_owned(),
            "-Wno-UNUSEDSIGNAL".to_owned(),
            "-Wno-UNDRIVEN".to_owned(),
            "-Wno-UNOPTFLAT".to_owned(),
            "-Wno-STMTDLY".to_owned(),
            "-Wno-CASEINCOMPLETE".to_owned(),
            "-Wno-SYMRSVDWORD".to_owned(),
            "-Wno-COMBDLY".to_owned(),
            "-Wno-TIMESCALEMOD".to_owned(),
            "-Wno-MULTIDRIVEN".to_owned(),
            "-y".to_owned(),
            rtl_dir.to_string_lossy().to_string(),
        ];
        for f in std::fs::read_dir(&rtl_dir)? {
            let path = f?.path();
            if is_verilog_like(&path) {
                args.push(path.to_string_lossy().to_string());
            }
        }
        // Strip VERILATOR_BIN from the children: Verilator's own Perl
        // frontend consumes that variable to pick its backend binary, so
        // leaving it pointed at a wrapper makes the wrapper re-exec
        // itself forever (a fork bomb). Compiled binaries ignore it, and
        // a wrapper self-selects its backend correctly without it.
        let status = Command::new(&verilator_bin)
            .args(&args)
            .env_remove("VERILATOR_BIN")
            .envs(
                verilator_root_env(&verilator_bin)
                    .into_iter()
                    .map(|root| ("VERILATOR_ROOT", root)),
            )
            .current_dir(tb_dir)
            .status()?;
        if !status.success() {
            return Err(CosimError::SimFailed(status));
        }
        let status = Command::new("make")
            .args([
                "-j",
                &num_cpus_str(),
                "-C",
                "obj_dir",
                "-f",
                &format!("V{top}.mk"),
                &format!("V{top}"),
            ])
            .env_remove("VERILATOR_BIN")
            .current_dir(tb_dir)
            .status()?;
        if !status.success() {
            return Err(CosimError::SimFailed(status));
        }

        let obj_dir = tb_dir.join("obj_dir");
        let mut found = None;
        for entry in std::fs::read_dir(&obj_dir)? {
            let entry = entry?;
            let path = entry.path();
            let looks_like_verilator_bin = entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.starts_with('V'));
            let is_executable = path.is_file()
                && std::fs::metadata(&path).is_ok_and(|m| {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        m.permissions().mode() & 0o111 != 0
                    }
                    #[cfg(not(unix))]
                    {
                        true
                    }
                });
            if looks_like_verilator_bin && is_executable {
                found = Some(path);
                break;
            }
        }
        let top =
            found.ok_or_else(|| CosimError::ToolNotFound("Verilator binary in obj_dir".into()))?;
        let mut cmd = Command::new(top);
        cmd.envs(xilinx_environ())
            .env(frt_shm::env::TAPA_DPI_CONFIG, ctx.dpi_config_json());
        configure_sim_command(&mut cmd);
        let child = cmd.spawn()?;
        Ok(child)
    }
}

struct VerilatorBuildGate {
    #[cfg(unix)]
    _file: std::fs::File,
}

impl VerilatorBuildGate {
    fn acquire() -> Result<Self> {
        #[cfg(unix)]
        {
            let file = super::acquire_exclusive_lock(&verilator_build_lock_path())?;
            Ok(Self { _file: file })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {})
        }
    }
}

fn verilator_build_lock_path() -> PathBuf {
    std::env::var_os(VERILATOR_BUILD_LOCK_ENV)
        .filter(|path| !path.is_empty())
        .map_or_else(
            || std::env::temp_dir().join("frt-verilator-build.lock"),
            PathBuf::from,
        )
}

fn num_cpus_str() -> String {
    std::thread::available_parallelism().map_or_else(|_| "4".into(), |n| n.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct XilinxFpIpConfig {
    dpi_func: String,
    latency: usize,
}

fn generate_xilinx_fp_ip_models(rtl_dir: &Path) -> Result<()> {
    let mut generated = HashSet::new();
    let ip_re = Regex::new(r"(?m)^\s*(\w+_ip)\s+\w+\s*\(")
        .map_err(|e| CosimError::Metadata(format!("regex compile failed: {e}")))?;

    for entry in std::fs::read_dir(rtl_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !is_verilog_like(&path) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        for ip_module in ip_re
            .captures_iter(&content)
            .filter_map(|caps| caps.get(1).map(|m| m.as_str().to_owned()))
        {
            if !generated.insert(ip_module.clone()) {
                continue;
            }
            let ip_v_path = rtl_dir.join(format!("{ip_module}.v"));
            let ip_sv_path = rtl_dir.join(format!("{ip_module}.sv"));
            if let Ok(existing) = std::fs::read_to_string(&ip_v_path) {
                if !existing.contains("`pragma protect") {
                    continue;
                }
            }
            if let Ok(existing) = std::fs::read_to_string(&ip_sv_path) {
                if !existing.contains("`pragma protect") {
                    continue;
                }
            }

            let model = parse_xilinx_fp_ip_model(&ip_module, rtl_dir)?;
            if let Some(model) = model {
                std::fs::write(
                    &ip_sv_path,
                    generate_xilinx_fp_ip_replacement(&ip_module, &model),
                )?;
            }
        }
    }

    Ok(())
}

/// Fix non-blocking assignments (`<=`) inside combinational `always @(*)`
/// blocks. HLS-generated RTL sometimes uses NBA in combinational logic, which
/// Verilator silently converts to blocking `=` with a COMBDLY warning. By
/// patching the source we ensure identical behavior across all simulators and
/// silence the warning.
fn fix_combinational_nba(rtl_dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(rtl_dir)? {
        let path = entry?.path();
        if !is_verilog_like(&path) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !content.contains("always @(*)") {
            continue;
        }
        let result = rewrite_combinational_nba(&content);
        if result != content {
            std::fs::write(&path, &result)?;
        }
    }
    Ok(())
}

fn rewrite_combinational_nba(content: &str) -> String {
    // Match NBA statements: `identifier <= expr;` but not comparisons like `(a <= b)`
    static NBA_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(\s+\w+)\s*<=\s*(.+;)$").unwrap());
    // Block keywords must match as whole words: identifiers like
    // `wire_begin` and keywords like `endcase`/`endfunction` contain
    // `begin`/`end` as substrings, and counting those corrupts the
    // depth — leaking the rewrite into sequential blocks below.
    static BEGIN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bbegin\b").unwrap());
    static END_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bend\b").unwrap());

    // A combinational block is either a `begin`/`end` region (tracked by
    // depth) or a single statement (rewrite ends with that statement).
    enum Comb {
        Outside,
        Block(i32),
        Statement,
    }

    let mut result = String::with_capacity(content.len());
    let mut state = Comb::Outside;
    for line in content.lines() {
        // Keywords in `// ...` comments must not count either.
        let code = line.split("//").next().unwrap_or(line);
        let trimmed = code.trim();
        let mut rewrite = false;
        match &mut state {
            Comb::Block(depth) => {
                rewrite = true;
                *depth += BEGIN_RE.find_iter(code).count() as i32;
                *depth -= END_RE.find_iter(code).count() as i32;
                if *depth <= 0 {
                    state = Comb::Outside;
                }
            }
            Comb::Statement => {
                rewrite = true;
                if code.contains(';') {
                    state = Comb::Outside;
                }
            }
            Comb::Outside => {
                if trimmed.starts_with("always") && trimmed.contains("@(*)") {
                    let depth = BEGIN_RE.find_iter(code).count() as i32
                        - END_RE.find_iter(code).count() as i32;
                    if depth > 0 {
                        state = Comb::Block(depth);
                    } else if code.contains(';') {
                        // Single-line form (`always @(*) a <= b;`): rewrite
                        // this line only and stay outside.
                        rewrite = true;
                    } else {
                        // Statement on the following line(s).
                        state = Comb::Statement;
                    }
                }
            }
        }
        // Replace NBA with blocking assignment inside combinational blocks.
        // A single-line `always @(*) a <= b;` needs the intra-line form too.
        let fixed = if rewrite {
            if NBA_RE.is_match(line) {
                NBA_RE.replace(line, "$1 = $2").into_owned()
            } else if trimmed.starts_with("always") {
                // `always @(*) a <= b;` does not match the anchored NBA_RE.
                static INLINE_NBA_RE: LazyLock<Regex> =
                    LazyLock::new(|| Regex::new(r"@\(\*\)\s*(\w+)\s*<=").unwrap());
                INLINE_NBA_RE.replace(line, "@(*) $1 =").into_owned()
            } else {
                line.to_owned()
            }
        } else {
            line.to_owned()
        };
        result.push_str(&fixed);
        result.push('\n');
    }
    result
}

fn is_verilog_like(path: &Path) -> bool {
    path.extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| matches!(x, "v" | "sv" | "vh"))
}

fn parse_xilinx_fp_ip_model(ip_module: &str, rtl_dir: &Path) -> Result<Option<XilinxFpIpConfig>> {
    if let Some(config) = parse_xilinx_fp_ip_tcl(&rtl_dir.join(format!("{ip_module}.tcl")))? {
        return Ok(Some(config));
    }
    Ok(detect_xilinx_fp_ip_model_from_name(ip_module))
}

fn parse_xilinx_fp_ip_tcl(tcl_path: &Path) -> Result<Option<XilinxFpIpConfig>> {
    let Ok(content) = std::fs::read_to_string(tcl_path) else {
        return Ok(None);
    };
    parse_xilinx_fp_ip_tcl_text(&content)
}

fn parse_xilinx_fp_ip_tcl_text(content: &str) -> Result<Option<XilinxFpIpConfig>> {
    if !content.contains("create_ip -name floating_point") {
        return Ok(None);
    }

    let config_re = Regex::new(r"CONFIG\.([A-Za-z0-9_]+)\s+([^\s\\]+)")
        .map_err(|e| CosimError::Metadata(format!("regex compile failed: {e}")))?;
    let mut config = HashMap::<String, String>::new();
    for caps in config_re.captures_iter(content) {
        let key = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        let value = caps.get(2).map(|m| m.as_str()).unwrap_or_default();
        config.insert(key.to_owned(), value.to_owned());
    }

    let precision = config
        .get("a_precision_type")
        .map_or_else(|| "single".into(), |s| s.to_ascii_lowercase());
    let is_double = precision == "double";

    let op = if let Some(operation_type) = config.get("operation_type") {
        let operation_type = operation_type.to_ascii_lowercase();
        if operation_type.contains("multiply") {
            Some("mul")
        } else if operation_type.contains("add") || operation_type.contains("subtract") {
            // For Add_Subtract IPs, check add_sub_value to distinguish add vs sub
            let add_sub = config.get("add_sub_value").map(|s| s.to_ascii_lowercase());
            if add_sub.as_deref() == Some("subtract") || add_sub.as_deref() == Some("sub") {
                Some("sub")
            } else {
                Some("add")
            }
        } else {
            None
        }
    } else {
        None
    };

    let Some(op) = op else {
        return Ok(None);
    };

    let dpi_func = match (is_double, op) {
        (false, "add") => "fp32_add",
        (false, "sub") => "fp32_sub",
        (false, "mul") => "fp32_mul",
        (true, "add") => "fp64_add",
        (true, "sub") => "fp64_sub",
        (true, "mul") => "fp64_mul",
        _ => return Ok(None),
    };
    let latency = match config.get("c_latency") {
        Some(s) => s.parse::<usize>().map_err(|e| {
            CosimError::Metadata(format!("invalid c_latency '{s}' in FP IP TCL: {e}"))
        })?,
        None => {
            return Err(CosimError::Metadata(
                "FP IP TCL missing CONFIG.c_latency".into(),
            ));
        }
    };

    Ok(Some(XilinxFpIpConfig {
        dpi_func: dpi_func.to_owned(),
        latency,
    }))
}

fn detect_xilinx_fp_ip_model_from_name(ip_module: &str) -> Option<XilinxFpIpConfig> {
    const FP_PATTERNS: &[&str] = &[
        "_fadd_", "_fadds_", "_fsub_", "_fsubs_", "_fmul_", "_fmuls_", "_dadd_", "_dadds_",
        "_dsub_", "_dsubs_", "_dmul_", "_dmuls_",
    ];
    let lower = ip_module.to_ascii_lowercase();
    if FP_PATTERNS.iter().any(|p| lower.contains(p)) {
        eprintln!(
            "frt-cosim: FP IP '{ip_module}' detected by name but no TCL with c_latency found; \
             skipping replacement",
        );
    }
    // No TCL available; cannot determine latency reliably.
    None
}

fn generate_xilinx_fp_ip_replacement(ip_module: &str, model: &XilinxFpIpConfig) -> String {
    let bit_width = if model.dpi_func.contains("64") {
        64
    } else {
        32
    };
    let ret_type = if bit_width == 64 {
        "longint unsigned"
    } else {
        "int unsigned"
    };
    let latency = model.latency;

    if latency == 0 {
        return format!(
            r#"`timescale 1ns/1ps

module {ip_module} (
    input  wire        aclk,
    input  wire        aclken,
    input  wire        s_axis_a_tvalid,
    input  wire [{data_hi}:0] s_axis_a_tdata,
    input  wire        s_axis_b_tvalid,
    input  wire [{data_hi}:0] s_axis_b_tdata,
    output wire        m_axis_result_tvalid,
    output wire [{data_hi}:0] m_axis_result_tdata
);

import "DPI-C" function {ret_type} {dpi_func}(
input {ret_type} a, input {ret_type} b);

assign m_axis_result_tdata = {dpi_func}(s_axis_a_tdata, s_axis_b_tdata);
assign m_axis_result_tvalid = s_axis_a_tvalid & s_axis_b_tvalid;

endmodule
"#,
            ip_module = ip_module,
            data_hi = bit_width - 1,
            ret_type = ret_type,
            dpi_func = model.dpi_func,
        );
    }

    let depth = latency;
    let depth_hi = depth - 1;
    format!(
        r#"`timescale 1ns/1ps

module {ip_module} (
    input  wire        aclk,
    input  wire        aclken,
    input  wire        s_axis_a_tvalid,
    input  wire [{data_hi}:0] s_axis_a_tdata,
    input  wire        s_axis_b_tvalid,
    input  wire [{data_hi}:0] s_axis_b_tdata,
    output wire        m_axis_result_tvalid,
    output wire [{data_hi}:0] m_axis_result_tdata
);

import "DPI-C" function {ret_type} {dpi_func}(
input {ret_type} a, input {ret_type} b);

reg [{data_hi}:0] pipe [0:{depth_hi}];
reg [{depth_hi}:0]  valid_pipe;

integer i;

always @(posedge aclk) begin
    if (aclken) begin
        pipe[0] <= {dpi_func}(s_axis_a_tdata, s_axis_b_tdata);
        valid_pipe[0] <= s_axis_a_tvalid & s_axis_b_tvalid;
        for (i = 1; i < {depth}; i = i + 1) begin
            pipe[i] <= pipe[i-1];
            valid_pipe[i] <= valid_pipe[i-1];
        end
    end
end

assign m_axis_result_tdata  = pipe[{depth_hi}];
assign m_axis_result_tvalid = valid_pipe[{depth_hi}];

endmodule
"#,
        ip_module = ip_module,
        data_hi = bit_width - 1,
        ret_type = ret_type,
        dpi_func = model.dpi_func,
        depth = depth,
        depth_hi = depth_hi,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nba_rewrite_stays_inside_combinational_block() {
        let src = "\
module m;
always @(*) begin
    a <= b;
end
always @(posedge clk) begin
    q <= d;
end
endmodule
";
        let out = rewrite_combinational_nba(src);
        assert!(out.contains("    a = b;"), "comb NBA rewritten:\n{out}");
        assert!(out.contains("    q <= d;"), "sequential NBA kept:\n{out}");
    }

    /// `begin` inside an identifier must not open a level: the depth
    /// never returning to zero leaks the rewrite into every sequential
    /// block below — silent RTL corruption.
    #[test]
    fn nba_rewrite_ignores_begin_inside_identifiers() {
        let src = "\
module m;
always @(*) begin
    state_begin <= next;
end
always @(posedge clk) begin
    q <= d;
end
endmodule
";
        let out = rewrite_combinational_nba(src);
        assert!(
            out.contains("    state_begin = next;"),
            "comb NBA rewritten:\n{out}"
        );
        assert!(out.contains("    q <= d;"), "sequential NBA kept:\n{out}");
    }

    /// The `end` inside `endcase` must not close the block early:
    /// statements after the case would silently keep their NBAs.
    #[test]
    fn nba_rewrite_survives_endcase() {
        let src = "\
module m;
always @(*) begin
    case (s)
        1'b0: x <= a;
    endcase
    y <= b;
end
endmodule
";
        let out = rewrite_combinational_nba(src);
        assert!(
            out.contains("        1'b0: x <= a;"),
            "case arm intact:\n{out}"
        );
        assert!(
            out.contains("    y = b;"),
            "post-case NBA rewritten:\n{out}"
        );
    }

    /// Keywords in comments must not perturb the depth either way.
    #[test]
    fn nba_rewrite_ignores_keywords_in_comments() {
        let src = "\
module m;
always @(*) begin
    // end of the hot path
    a <= b;
end
always @(posedge clk) begin
    q <= d;
end
endmodule
";
        let out = rewrite_combinational_nba(src);
        assert!(out.contains("    a = b;"), "comb NBA rewritten:\n{out}");
        assert!(out.contains("    q <= d;"), "sequential NBA kept:\n{out}");
    }

    /// A single-line `always @(*) a <= b;` must be rewritten on that line,
    /// and the rewrite must not leak one line past it into sequential logic.
    #[test]
    fn nba_rewrite_single_line_form_does_not_leak() {
        let src = "\
module m;
always @(*) a <= b;
always @(posedge clk) begin
    q <= d;
end
endmodule
";
        let out = rewrite_combinational_nba(src);
        assert!(out.contains("always @(*) a = b;"), "comb NBA rewritten:\n{out}");
        assert!(out.contains("    q <= d;"), "sequential NBA kept:\n{out}");
    }

    /// The statement-on-next-line form rewrites exactly that statement.
    #[test]
    fn nba_rewrite_next_line_form_stops_after_the_statement() {
        let src = "\
module m;
always @(*)
    a <= b;
always @(posedge clk) begin
    q <= d;
end
endmodule
";
        let out = rewrite_combinational_nba(src);
        assert!(out.contains("    a = b;"), "comb NBA rewritten:\n{out}");
        assert!(out.contains("    q <= d;"), "sequential NBA kept:\n{out}");
    }

    #[test]
    fn resolve_verilator_bin_prefers_env_override() {
        let bin = PathBuf::from("/tmp/custom/verilator");
        assert_eq!(
            resolve_verilator_bin(Some(bin.clone()), None).expect("resolve bin"),
            bin
        );
    }

    #[test]
    fn resolve_verilator_bin_anchors_relative_runfiles_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let runfiles_root = tmp.path().join("bandwidth-verilator-zipsim.runfiles");
        let rel_bin = PathBuf::from("verilator+/bin/verilator");
        let abs_bin = runfiles_root.join(&rel_bin);
        std::fs::create_dir_all(abs_bin.parent().expect("bin parent")).expect("mkdir");
        std::fs::write(&abs_bin, []).expect("write bin");

        assert_eq!(
            resolve_verilator_bin(Some(rel_bin), Some(runfiles_root)).expect("resolve bin"),
            abs_bin
        );
    }

    #[test]
    fn verilator_root_env_uses_bin_parent_parent_when_valid() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("verilator+");
        let bin = root.join("bin/verilator");
        // Create the layout Verilator expects: $ROOT/include/verilated.h
        std::fs::create_dir_all(root.join("include")).expect("mkdir include");
        std::fs::write(root.join("include/verilated.h"), []).expect("write verilated.h");
        std::fs::create_dir_all(bin.parent().expect("bin parent")).expect("mkdir bin");
        std::fs::write(&bin, []).expect("write bin");
        assert_eq!(verilator_root_env(&bin).expect("root"), root);
    }

    #[test]
    fn verilator_root_env_skips_system_install() {
        // System installs (Homebrew, apt) have include/verilated.h under
        // share/verilator/, not directly under the prefix. Verilator knows
        // its own root in this case, so we should not override it.
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("homebrew_prefix");
        let bin = root.join("bin/verilator");
        // Homebrew layout: share/verilator/include/verilated.h, NOT include/verilated.h
        std::fs::create_dir_all(root.join("share/verilator/include")).expect("mkdir");
        std::fs::write(root.join("share/verilator/include/verilated.h"), []).expect("write");
        std::fs::create_dir_all(bin.parent().expect("bin parent")).expect("mkdir bin");
        std::fs::write(&bin, []).expect("write bin");
        assert!(verilator_root_env(&bin).is_none() || std::env::var_os("VERILATOR_ROOT").is_some());
    }

    #[test]
    fn parses_xilinx_floating_point_tcl() {
        let tcl = r"
create_ip -name floating_point -version 7.1 -vendor xilinx.com -library ip -module_name Add_fadd_32ns_32ns_32_7_full_dsp_1_ip
set_property -dict [list CONFIG.a_precision_type Single \
                          CONFIG.add_sub_value Add \
                          CONFIG.c_latency 5 \
                          CONFIG.operation_type Add_Subtract] -objects [get_ips Add_fadd_32ns_32ns_32_7_full_dsp_1_ip] -quiet
";
        let model = parse_xilinx_fp_ip_tcl_text(tcl)
            .expect("parse tcl")
            .expect("fp model");
        assert_eq!(model.dpi_func, "fp32_add");
        assert_eq!(model.latency, 5);
    }

    #[test]
    fn generates_behavioral_replacement_for_floating_point_ip() {
        let model = XilinxFpIpConfig {
            dpi_func: "fp32_mul".into(),
            latency: 5,
        };
        let replacement =
            generate_xilinx_fp_ip_replacement("ProcElem_fmul_32ns_32ns_32_4_max_dsp_1_ip", &model);
        assert!(replacement.contains("module ProcElem_fmul_32ns_32ns_32_4_max_dsp_1_ip"));
        assert!(replacement.contains("import \"DPI-C\" function int unsigned fp32_mul"));
        assert!(replacement.contains("m_axis_result_tvalid"));
    }

    #[test]
    fn generates_behavioral_replacement_files_in_rtl_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let wrapper = include_str!("../../testdata/Add_fadd_32ns_32ns_32_7_full_dsp_1.v");
        let tcl = include_str!("../../testdata/Add_fadd_32ns_32ns_32_7_full_dsp_1_ip.tcl");
        std::fs::write(
            tmp.path().join("Add_fadd_32ns_32ns_32_7_full_dsp_1.v"),
            wrapper,
        )
        .expect("write wrapper");
        std::fs::write(
            tmp.path().join("Add_fadd_32ns_32ns_32_7_full_dsp_1_ip.tcl"),
            tcl,
        )
        .expect("write tcl");

        generate_xilinx_fp_ip_models(tmp.path()).expect("generate models");
        let model =
            std::fs::read_to_string(tmp.path().join("Add_fadd_32ns_32ns_32_7_full_dsp_1_ip.sv"))
                .expect("read generated model");
        assert!(model.contains("module Add_fadd_32ns_32ns_32_7_full_dsp_1_ip"));
        assert!(model.contains("import \"DPI-C\" function int unsigned fp32_add"));
    }
}
