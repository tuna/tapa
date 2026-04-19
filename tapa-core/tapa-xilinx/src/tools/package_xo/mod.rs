//! `.xo` packaging: kernel.xml + Vivado `package_xo` + ZIP redaction.
//!
//! Ports `tapa/backend/xilinx_tools.py::PackageXo`,
//! `tapa/verilog/xilinx/pack.py`, and `tapa/program/pack.py` redaction.
//! The kernel.xml emission is delegated to
//! `platform::kernel_xml::emit_kernel_xml`; the full Vivado-backed
//! build lands alongside `run_vivado`.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use zip::write::SimpleFileOptions;

use crate::error::{Result, XilinxError};
use crate::platform::device::DeviceInfo;
use crate::platform::kernel_xml::{emit_kernel_xml, KernelXmlArgs, KernelXmlPort, PortCategory};
use crate::runtime::process::ToolRunner;
use crate::tools::vivado::{run_vivado, VivadoJob};

const S_AXI_NAME: &str = "s_axi_control";
const M_AXI_PREFIX: &str = "m_axi_";

/// Ports `tapa/backend/xilinx_tools.py::PACKAGEXO_COMMANDS` byte-for-byte.
///
/// `{top_name}`, `{bus_ifaces}`, `{cpp_kernels}`, `{part_num}` placeholders
/// are substituted by `format_package_xo_tcl`. All other braces are escaped
/// (`{{`/`}}`) so the Python `.format` semantics carry over cleanly.
const PACKAGE_XO_TCL: &str = r#"
# Paths passed via tclargs for remote execution path rewriting:
# argv[0] = tmpdir, argv[1] = hdl_dir, argv[2] = xo_file, argv[3] = kernel_xml
set tmpdir [lindex $argv 0]
set hdl_dir [lindex $argv 1]
set xo_file [lindex $argv 2]
set kernel_xml_path [lindex $argv 3]
set tmp_ip_dir "$tmpdir/tmp_ip_dir"
set tmp_project "$tmpdir/tmp_project"

create_project -force kernel_pack ${tmp_project}{part_num}
add_files [glob -nocomplain $hdl_dir/* $hdl_dir/*/* $hdl_dir/*/*/* \
        $hdl_dir/*/*/*/* $hdl_dir/*/*/*/*/*]
foreach tcl_file [glob -nocomplain $hdl_dir/*.tcl $hdl_dir/*/*.tcl] {
  source ${tcl_file}
}
set_property top {top_name} [current_fileset]
update_compile_order -fileset sources_1
update_compile_order -fileset sim_1
ipx::package_project -root_dir ${tmp_ip_dir} -vendor tapa \
        -library xrtl -taxonomy /KernelIP -import_files -set_current false
ipx::unload_core ${tmp_ip_dir}/component.xml
ipx::edit_ip_in_project -upgrade true -name tmp_edit_project \
        -directory ${tmp_ip_dir} ${tmp_ip_dir}/component.xml
set_property core_revision 2 [ipx::current_core]
foreach up [ipx::get_user_parameters] {
  ipx::remove_user_parameter [get_property NAME ${up}] [ipx::current_core]
}
set_property sdx_kernel true [ipx::current_core]
set_property sdx_kernel_type rtl [ipx::current_core]
ipx::create_xgui_files [ipx::current_core]
{bus_ifaces}
set_property xpm_libraries {XPM_CDC XPM_MEMORY XPM_FIFO} [ipx::current_core]
set_property supported_families { } [ipx::current_core]
set_property auto_family_support_level level_2 [ipx::current_core]
ipx::update_checksums [ipx::current_core]
ipx::save_core [ipx::current_core]
close_project -delete

package_xo -force -xo_path "$xo_file" -kernel_name {top_name} \
        -ip_directory ${tmp_ip_dir} -kernel_xml $kernel_xml_path{cpp_kernels}
"#;

const BUS_IFACE_TCL: &str = "
ipx::associate_bus_interfaces -busif {iface} -clock ap_clk [ipx::current_core]
";

const BUS_PARAM_TCL: &str =
    "set_property value {value} [ipx::add_bus_parameter {key} [ipx::get_bus_interfaces {iface}]]
";

#[derive(Debug, Clone)]
pub struct PackageXoInputs {
    pub top_name: String,
    /// Directory of Verilog/SystemVerilog sources glob'd by the TCL.
    pub hdl_dir: PathBuf,
    pub device_info: DeviceInfo,
    pub clock_period: String,
    pub kernel_xml: KernelXmlArgs,
    pub kernel_out_path: PathBuf,
    /// Optional `-kernel_files` C++ sources appended to `package_xo`.
    pub cpp_kernels: Vec<PathBuf>,
    /// Optional per-port bus parameters, keyed by m_axi port name (no prefix).
    pub m_axi_params: Vec<(String, Vec<(String, String)>)>,
    /// S_AXI interfaces to associate; defaults to `[s_axi_control]`.
    pub s_axi_ifaces: Vec<String>,
    /// Extra HLS report files to append under the packaged `.xo`'s
    /// `report/` tree before redaction. Each entry is `(source_path,
    /// archive_name)` — the archive name is taken verbatim, so the
    /// caller is responsible for namespacing per-task reports (e.g.
    /// `report/<task>/<file>`). Mirrors Python's `PackageXo.__init__`
    /// which appends per-task `report/<task-rel>/<file>` entries so
    /// downstream inspection tooling can disambiguate same-basename
    /// reports across tasks (`csynth.rpt`, `csynth.xml`, …). Empty →
    /// skip the bundle step.
    pub report_paths: Vec<(PathBuf, String)>,
}

impl PackageXoInputs {
    #[must_use]
    pub fn default_s_axi() -> Vec<String> {
        vec![S_AXI_NAME.to_string()]
    }
}

fn m_axi_port_names(args: &KernelXmlArgs) -> Vec<String> {
    args.ports
        .iter()
        .filter(|p: &&KernelXmlPort| p.category == PortCategory::MAxi)
        .map(|p| p.name.clone())
        .collect()
}

#[allow(
    clippy::literal_string_with_formatting_args,
    reason = "{iface}/{key}/{value} are literal TCL template placeholders, not format-args"
)]
fn render_bus_ifaces(
    s_axi: &[String],
    m_axi: &[String],
    params: &[(String, Vec<(String, String)>)],
) -> String {
    let mut out = String::new();
    for iface in s_axi {
        out.push_str(&BUS_IFACE_TCL.replace("{iface}", iface));
    }
    let param_map: std::collections::HashMap<&str, &[(String, String)]> =
        params.iter().map(|(n, kv)| (n.as_str(), kv.as_slice())).collect();
    for name in m_axi {
        let full = format!("{M_AXI_PREFIX}{name}");
        out.push_str(&BUS_IFACE_TCL.replace("{iface}", &full));
        if let Some(kv) = param_map.get(name.as_str()) {
            for (k, v) in *kv {
                out.push_str(
                    &BUS_PARAM_TCL
                        .replace("{iface}", &full)
                        .replace("{key}", k)
                        .replace("{value}", v),
                );
            }
        }
    }
    out
}

fn render_cpp_kernels(kernels: &[PathBuf]) -> String {
    let mut out = String::new();
    for k in kernels {
        out.push_str(" -kernel_files ");
        out.push_str(&k.display().to_string());
    }
    out
}

#[allow(
    clippy::literal_string_with_formatting_args,
    reason = "{top_name}/{bus_ifaces}/{cpp_kernels}/{part_num} are literal TCL template placeholders"
)]
fn format_package_xo_tcl(
    top_name: &str,
    bus_ifaces: &str,
    cpp_kernels: &str,
    part_num: &str,
) -> String {
    let part_arg = if part_num.is_empty() {
        String::new()
    } else {
        format!(" -part {part_num}")
    };
    PACKAGE_XO_TCL
        .replace("{top_name}", top_name)
        .replace("{bus_ifaces}", bus_ifaces)
        .replace("{cpp_kernels}", cpp_kernels)
        .replace("{part_num}", &part_arg)
}

/// Build the `.xo` for the given inputs using the provided runner.
///
/// Ports `tapa/backend/xilinx_tools.py::PackageXo` + `tapa/verilog/xilinx/pack.py::pack`:
///
/// 1. Allocate a staging tempdir and emit `kernel.xml` into it.
/// 2. Format `PACKAGE_XO_TCL` with the kernel's `bus_ifaces`, `cpp_kernels`,
///    and `-part` argument, and invoke Vivado via [`run_vivado`].
/// 3. Require that Vivado has produced the `.xo` at `kernel_out_path`.
/// 4. Run [`redact_xo`] on the output so two invocations on the same
///    inputs are byte-equal.
///
/// `tclargs` to Vivado: `$tmpdir $hdl_dir $xo_file $kernel_xml_path`.
pub fn pack_xo(runner: &dyn ToolRunner, inputs: &PackageXoInputs) -> Result<PathBuf> {
    let out = pack_xo_without_redaction(runner, inputs)?;
    // Python parity: bundle the HLS report files (`self.report_paths`
    // + `report/*_csynth.xml`) into the packaged `.xo` before the
    // reproducibility redaction pass. Downstream inspection tooling
    // reads these archived reports; the previous implementation
    // redacted the raw Vivado `.xo` and dropped them.
    if !inputs.report_paths.is_empty() {
        bundle_report_paths_into_xo(&out, &inputs.report_paths)?;
    }
    redact_xo(&out)?;
    Ok(out)
}

/// Append each report into the `.xo` under its caller-provided archive
/// name, matching Python's `PackageXo.__init__` bundling step. Any
/// existing archive entry with the same name is overwritten so callers
/// can use task-relative names (e.g. `report/<task>/csynth.xml`)
/// without colliding with the basename layout the raw `.xo` already
/// carries.
fn bundle_report_paths_into_xo(
    xo: &std::path::Path,
    report_paths: &[(PathBuf, String)],
) -> Result<()> {
    use std::io::{Read, Write};
    if report_paths.is_empty() {
        return Ok(());
    }
    let raw = std::fs::read(xo)?;
    let mut z_in = zip::ZipArchive::new(std::io::Cursor::new(raw))
        .map_err(|e| XilinxError::XoRedaction(format!("open xo for bundling: {e}")))?;
    let tmp = tempfile::NamedTempFile::new_in(
        xo.parent().unwrap_or_else(|| std::path::Path::new(".")),
    )?;
    let written: std::collections::HashSet<&str> =
        report_paths.iter().map(|(_, name)| name.as_str()).collect();
    {
        let mut z_out = zip::ZipWriter::new(tmp.reopen()?);
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for i in 0..z_in.len() {
            let mut entry = z_in
                .by_index(i)
                .map_err(|e| XilinxError::XoRedaction(format!("read xo entry {i}: {e}")))?;
            if written.contains(entry.name()) {
                continue;
            }
            z_out
                .start_file(entry.name().to_owned(), opts)
                .map_err(|e| XilinxError::XoRedaction(format!("start entry: {e}")))?;
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            z_out.write_all(&buf)?;
        }
        for (rpt, name) in report_paths {
            if !rpt.is_file() {
                continue;
            }
            z_out
                .start_file(name.clone(), opts)
                .map_err(|e| XilinxError::XoRedaction(format!("bundle entry: {e}")))?;
            z_out.write_all(&std::fs::read(rpt)?)?;
        }
        z_out
            .finish()
            .map_err(|e| XilinxError::XoRedaction(format!("finish bundled xo: {e}")))?;
    }
    tmp.persist(xo).map_err(|e| {
        XilinxError::XoRedaction(format!("persist bundled xo: {e}"))
    })?;
    Ok(())
}

/// Same as [`pack_xo`] but returns the raw Vivado-produced `.xo`
/// without running the reproducibility redaction pass. Primarily
/// useful for parity tests that need a pre-redaction artifact to
/// hand to an alternate redactor (e.g. Python's `_redact_and_zip`)
/// for direct cross-language comparison.
pub fn pack_xo_without_redaction(
    runner: &dyn ToolRunner,
    inputs: &PackageXoInputs,
) -> Result<PathBuf> {
    if !inputs.hdl_dir.is_dir() {
        return Err(XilinxError::KernelXml(format!(
            "pack_xo hdl_dir does not exist: {}",
            inputs.hdl_dir.display()
        )));
    }
    // The Vivado job runs with `cwd = tmp.path()`, so a relative
    // `--output` would end up inside the temp dir and vanish after
    // run_vivado returns (and the downstream `is_file` / redaction
    // check would miss it or pick up a stale file from the caller's
    // cwd). Absolutize before wiring the TCL args and the download
    // list so remote + local paths agree on one absolute target.
    let kernel_out_path = if inputs.kernel_out_path.is_absolute() {
        inputs.kernel_out_path.clone()
    } else {
        std::env::current_dir()?.join(&inputs.kernel_out_path)
    };
    let tmp = tempfile::tempdir()?;
    let kernel_xml_path = tmp.path().join("kernel.xml");
    let xml = emit_kernel_xml(&inputs.kernel_xml)?;
    std::fs::write(&kernel_xml_path, xml.as_bytes())?;

    let s_axi = if inputs.s_axi_ifaces.is_empty() {
        PackageXoInputs::default_s_axi()
    } else {
        inputs.s_axi_ifaces.clone()
    };
    let m_axi = m_axi_port_names(&inputs.kernel_xml);
    let bus_ifaces = render_bus_ifaces(&s_axi, &m_axi, &inputs.m_axi_params);
    let cpp_kernels = render_cpp_kernels(&inputs.cpp_kernels);
    let tcl = format_package_xo_tcl(
        &inputs.top_name,
        &bus_ifaces,
        &cpp_kernels,
        &inputs.device_info.part_num,
    );

    if let Some(parent) = kernel_out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tclargs = [
        tmp.path().display().to_string(),
        inputs.hdl_dir.display().to_string(),
        kernel_out_path.display().to_string(),
        kernel_xml_path.display().to_string(),
    ];

    let mut job = VivadoJob::new(tcl);
    job.work_dir = Some(tmp.path().to_path_buf());
    job.uploads = vec![
        inputs.hdl_dir.clone(),
        tmp.path().to_path_buf(),
        kernel_xml_path,
    ];
    if let Some(parent) = kernel_out_path.parent() {
        job.downloads = vec![parent.to_path_buf()];
    }
    job.tclargs = tclargs.to_vec();

    let _out = run_vivado(runner, &job)?;
    if !kernel_out_path.is_file() {
        return Err(XilinxError::XoRedaction(format!(
            "pack_xo: Vivado returned success but {} is missing",
            kernel_out_path.display()
        )));
    }
    Ok(kernel_out_path)
}

fn redact_rpt(text: &str) -> String {
    // Matches Python's `Date:           <Day Mon DD HH:MM:SS YYYY>` pattern.
    let re = regex::Regex::new("Date:           ... ... .. ..:..:.. ....")
        .expect("static regex compiles");
    re.replace_all(text, "Date:           Tue Jan 01 00:00:00 1980")
        .into_owned()
}

fn redact_xml_payload(text: &str) -> String {
    let re_time = regex::Regex::new(
        "<xilinx:coreCreationDateTime>....-..-..T..:..:..Z</xilinx:coreCreationDateTime>",
    )
    .expect("static regex compiles");
    let step1 = re_time.replace_all(
        text,
        "<xilinx:coreCreationDateTime>1980-01-01T00:00:00Z</xilinx:coreCreationDateTime>",
    );

    let re_src = regex::Regex::new("<SourceLocation>.*/(cpp/[^<]*)</SourceLocation>")
        .expect("static regex compiles");
    let step2 = re_src.replace_all(&step1, "<SourceLocation>$1</SourceLocation>");

    // Python matches 32 arbitrary characters inside `ProjectID="..."`
    // (see `tapa/program/pack.py::_redact_xml`: 32 literal dots, and
    // `.` in `re` defaults to any-char-except-newline). Rust's
    // `regex` has the same default, so `.{32}` gives byte-for-byte
    // parity. Restricting to hex would miss valid XML payloads with
    // non-hex project identifiers.
    let re_pid = regex::Regex::new("ProjectID=\".{32}\"")
        .expect("static regex compiles");
    re_pid
        .replace_all(&step2, "ProjectID=\"0123456789abcdef0123456789abcdef\"")
        .into_owned()
}

/// Rewrite a `.xo` ZIP in place so two invocations on the same inputs
/// produce semantically-equal outputs.
///
/// Matches the Python `_redact_and_zip` / `_redact_rpt` /
/// `_redact_xml` triple in `tapa/program/pack.py`:
///
///   - ZIP timestamps are zeroed to the MS-DOS epoch.
///   - `*.rpt` `Date:` lines are rewritten to the epoch.
///   - `*.xml` entries have `xilinx:coreCreationDateTime`,
///     `SourceLocation` absolute paths, and `ProjectID` redacted.
///
/// Idempotent.
pub fn redact_xo(path: &std::path::Path) -> Result<()> {
    let bytes = std::fs::read(path)?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| XilinxError::XoRedaction(format!("read zip: {e}")))?;
    let mut out = std::io::Cursor::new(Vec::<u8>::new());
    {
        let mut writer = zip::ZipWriter::new(&mut out);
        let mut names: Vec<String> = (0..archive.len())
            .filter_map(|i| archive.by_index(i).ok().map(|e| e.name().to_string()))
            .collect();
        names.sort();
        for name in &names {
            let mut entry = archive
                .by_name(name)
                .map_err(|e| XilinxError::XoRedaction(format!("entry {name}: {e}")))?;
            let opts = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .last_modified_time(zip::DateTime::default());
            let mut buf = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut buf)?;
            let redacted: Vec<u8> = if name.ends_with(".rpt") {
                match std::str::from_utf8(&buf) {
                    Ok(text) => redact_rpt(text).into_bytes(),
                    Err(_) => buf,
                }
            } else if name.ends_with(".xml") {
                match std::str::from_utf8(&buf) {
                    Ok(text) => redact_xml_payload(text).into_bytes(),
                    Err(_) => buf,
                }
            } else {
                buf
            };
            writer
                .start_file(name.clone(), opts)
                .map_err(|e| XilinxError::XoRedaction(format!("start: {e}")))?;
            writer.write_all(&redacted)?;
        }
        writer
            .finish()
            .map_err(|e| XilinxError::XoRedaction(format!("finish: {e}")))?;
    }
    out.seek(SeekFrom::Start(0))?;
    std::fs::write(path, out.into_inner())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::process::MockToolRunner;

    fn minimal_inputs(hdl_dir: PathBuf, kernel_out_path: PathBuf) -> PackageXoInputs {
        PackageXoInputs {
            top_name: "k".into(),
            hdl_dir,
            device_info: DeviceInfo {
                part_num: "xcu250-figd2104-2L-e".into(),
                clock_period: "3.33".into(),
            },
            clock_period: "3.33".into(),
            kernel_xml: KernelXmlArgs {
                top_name: "k".into(),
                clock_period: "3.33".into(),
                ports: vec![KernelXmlPort {
                    name: "gmem0".into(),
                    category: PortCategory::MAxi,
                    width: 512,
                    port: String::new(),
                    ctype: "ap_uint<512>".into(),
                }],
            },
            kernel_out_path,
            cpp_kernels: vec![],
            m_axi_params: vec![],
            s_axi_ifaces: PackageXoInputs::default_s_axi(),
            report_paths: vec![],
        }
    }

    /// P1 regression: a relative `--output` path must be absolutized
    /// before reaching Vivado; otherwise the TCL writes the `.xo`
    /// into the per-invocation temp `cwd` while the post-run
    /// existence check looks in the caller's cwd.
    #[test]
    fn relative_xo_output_is_absolutized_for_tclargs() {
        use crate::runtime::process::ToolOutput;
        let tmp = tempfile::tempdir().unwrap();
        let hdl_dir = tmp.path().join("hdl");
        std::fs::create_dir_all(&hdl_dir).unwrap();
        std::fs::write(hdl_dir.join("top.v"), b"// stub\n").unwrap();
        // Scope current-dir into the tmp so a relative output still
        // lands in a writable place.
        let orig_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        // Stage a minimal .xo the mock runner will "produce".
        let staged = tmp.path().join("__staged.xo");
        write_xo(&staged, &[("stub.txt", "ok")]);
        let staged_bytes = std::fs::read(&staged).unwrap();

        let runner = MockToolRunner::new();
        runner.push_ok("vivado", ToolOutput::default());
        let expected_abs = tmp.path().canonicalize().unwrap().join("out.xo");
        runner.attach_download(&expected_abs, staged_bytes);

        let inputs = minimal_inputs(hdl_dir, PathBuf::from("out.xo"));
        let out = pack_xo(&runner, &inputs).unwrap();

        std::env::set_current_dir(orig_cwd).unwrap();
        assert!(
            out.is_absolute(),
            "pack_xo must return an absolute path; got `{}`",
            out.display(),
        );
        // The Vivado invocation must have received the absolute form.
        let call = &runner.calls()[0];
        let arg = call
            .args
            .iter()
            .find(|a| a.ends_with("out.xo"))
            .expect("tclargs must mention out.xo");
        assert!(
            std::path::Path::new(arg).is_absolute(),
            "tclargs .xo path must be absolute; got `{arg}`",
        );
    }

    #[test]
    fn missing_hdl_dir_is_rejected() {
        let runner = MockToolRunner::new();
        let inputs = minimal_inputs(
            PathBuf::from("/nonexistent/tapa-pack-xo-hdl"),
            PathBuf::from("/tmp/k.xo"),
        );
        let err = pack_xo(&runner, &inputs).unwrap_err();
        assert!(matches!(err, XilinxError::KernelXml(_)));
    }

    #[test]
    fn pack_xo_drives_vivado_and_redacts() {
        use crate::runtime::process::ToolOutput;
        let tmp = tempfile::tempdir().unwrap();
        let hdl_dir = tmp.path().join("hdl");
        std::fs::create_dir_all(&hdl_dir).unwrap();
        std::fs::write(hdl_dir.join("top.v"), b"// stub RTL\n").unwrap();
        let xo_path = tmp.path().join("k.xo");

        // Stage the synthetic .xo we expect Vivado to produce (pre-redaction).
        let staged = tmp.path().join("staged.xo");
        write_xo(
            &staged,
            &[(
                "ip/meta.xml",
                "<xilinx:coreCreationDateTime>2024-05-17T09:15:30Z</xilinx:coreCreationDateTime>",
            )],
        );
        let staged_bytes = std::fs::read(&staged).unwrap();

        let runner = MockToolRunner::new();
        runner.push_ok("vivado", ToolOutput::default());
        runner.attach_download(xo_path.clone(), staged_bytes);

        let inputs = minimal_inputs(hdl_dir, xo_path.clone());
        let out = pack_xo(&runner, &inputs).unwrap();
        assert_eq!(out, xo_path);

        // Vivado invocation recorded with -tclargs and the xo path.
        let call = &runner.calls()[0];
        assert_eq!(call.program, "vivado");
        assert!(call.args.iter().any(|a| a == "-tclargs"));
        assert!(call
            .args
            .iter()
            .any(|a| a == &xo_path.display().to_string()));
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(
            std::fs::read(&xo_path).unwrap(),
        ))
        .unwrap();
        let mut body = String::new();
        z.by_name("ip/meta.xml")
            .unwrap()
            .read_to_string(&mut body)
            .unwrap();
        assert!(body.contains("1980-01-01T00:00:00Z"));
    }

    #[test]
    fn format_package_xo_tcl_substitutes_placeholders() {
        let tcl = format_package_xo_tcl(
            "my_kernel",
            "\n# ifaces\n",
            " -kernel_files /tmp/x.cpp",
            "xcu250-figd2104-2L-e",
        );
        assert!(tcl.contains("set_property top my_kernel"));
        assert!(tcl.contains("-part xcu250-figd2104-2L-e"));
        assert!(tcl.contains("# ifaces"));
        assert!(tcl.contains("-kernel_files /tmp/x.cpp"));
        // Nothing left unsubstituted.
        assert!(!tcl.contains("{top_name}"));
        assert!(!tcl.contains("{bus_ifaces}"));
        assert!(!tcl.contains("{cpp_kernels}"));
        assert!(!tcl.contains("{part_num}"));
    }

    #[test]
    fn render_bus_ifaces_includes_m_axi_prefix_and_params() {
        let s = render_bus_ifaces(
            &["s_axi_control".into()],
            &["gmem0".into()],
            &[("gmem0".into(), vec![("OFFSET".into(), "SLAVE".into())])],
        );
        assert!(s.contains("-busif s_axi_control"));
        assert!(s.contains("-busif m_axi_gmem0"));
        assert!(s.contains("m_axi_gmem0") && s.contains("OFFSET") && s.contains("SLAVE"));
    }

    fn write_xo(path: &std::path::Path, entries: &[(&str, &str)]) {
        let f = std::fs::File::create(path).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts = SimpleFileOptions::default();
        for (name, body) in entries {
            zw.start_file((*name).to_string(), opts).unwrap();
            zw.write_all(body.as_bytes()).unwrap();
        }
        zw.finish().unwrap();
    }

    #[test]
    fn redact_xo_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("k.xo");
        write_xo(&path, &[("hello.txt", "hi")]);
        redact_xo(&path).unwrap();
        let first = std::fs::read(&path).unwrap();
        redact_xo(&path).unwrap();
        let second = std::fs::read(&path).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn redact_xml_rewrites_timestamp_sourceloc_projectid() {
        let input = r#"<root>
  <xilinx:coreCreationDateTime>2024-05-17T09:15:30Z</xilinx:coreCreationDateTime>
  <SourceLocation>/work/alice/build/cpp/foo.cc</SourceLocation>
  <meta ProjectID="deadbeefcafebabe0123456789abcdef"/>
</root>"#;
        let out = redact_xml_payload(input);
        assert!(out.contains("<xilinx:coreCreationDateTime>1980-01-01T00:00:00Z"));
        assert!(out.contains("<SourceLocation>cpp/foo.cc</SourceLocation>"));
        assert!(out.contains(r#"ProjectID="0123456789abcdef0123456789abcdef""#));
    }

    #[test]
    fn redact_rpt_rewrites_date_line() {
        let input = "\
Copyright ...\n\
Date:           Fri Mar 14 10:20:30 2025\n\
--+--\n";
        let out = redact_rpt(input);
        assert!(out.contains("Date:           Tue Jan 01 00:00:00 1980"));
    }

    #[test]
    fn redact_xo_applies_payload_redaction() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("k.xo");
        write_xo(
            &path,
            &[(
                "ip/meta.xml",
                "<xilinx:coreCreationDateTime>2024-05-17T09:15:30Z</xilinx:coreCreationDateTime>",
            )],
        );
        redact_xo(&path).unwrap();
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(std::fs::read(&path).unwrap()))
            .unwrap();
        let mut body = String::new();
        z.by_name("ip/meta.xml")
            .unwrap()
            .read_to_string(&mut body)
            .unwrap();
        assert!(body.contains("1980-01-01T00:00:00Z"));
    }
}
