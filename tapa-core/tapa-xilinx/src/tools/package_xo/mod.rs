//! `.xo` packaging: kernel.xml + Vivado `package_xo` + ZIP redaction.
//!
//! Implements kernel.xml emission, Vivado package generation, and archive
//! redaction. The kernel.xml emission is delegated to
//! `platform::kernel_xml::emit_kernel_xml`; the full Vivado-backed
//! build lands alongside `run_vivado`.

use std::io::{Read, Seek, SeekFrom, Write};

use camino::Utf8PathBuf;
use quick_xml::events::BytesStart;
use zip::write::SimpleFileOptions;

use crate::error::{Result, XilinxError};
use crate::platform::device::DeviceInfo;
use crate::platform::kernel_xml::{emit_kernel_xml, KernelXmlArgs, KernelXmlPort, PortCategory};
use crate::runtime::process::ToolRunner;
use crate::tools::vivado::{run_vivado, VivadoJob};

const S_AXI_NAME: &str = "s_axi_control";
const M_AXI_PREFIX: &str = "m_axi_";

/// Implements byte-for-byte.
///
/// `{top_name}`, `{bus_ifaces}`, `{cpp_kernels}`, `{part_num}` placeholders
/// are substituted by `format_package_xo_tcl`. All other braces are escaped
/// (`{{`/`}}`) so the `.format` semantics carry over cleanly.
#[derive(Debug, Clone)]
pub struct PackageXoInputs {
    pub top_name: String,
    /// Directory of Verilog/SystemVerilog sources glob'd by the TCL.
    pub hdl_dir: Utf8PathBuf,
    pub device_info: DeviceInfo,
    pub clock_period: String,
    pub kernel_xml: KernelXmlArgs,
    pub kernel_out_path: Utf8PathBuf,
    /// Optional `-kernel_files` C++ sources appended to `package_xo`.
    pub cpp_kernels: Vec<Utf8PathBuf>,
    /// Optional per-port bus parameters, keyed by m_axi port name (no prefix).
    pub m_axi_params: Vec<(String, Vec<(String, String)>)>,
    /// S_AXI interfaces to associate; defaults to `[s_axi_control]`.
    pub s_axi_ifaces: Vec<String>,
    /// Extra HLS report files to append under the packaged `.xo`'s
    /// `report/` tree before redaction. Each entry is `(source_path,
    /// archive_name)` — the archive name is taken verbatim, so the
    /// caller is responsible for namespacing per-task reports (e.g.
    /// `report/<task>/<file>`). Mirrors `PackageXo.__init__`
    /// which appends per-task `report/<task-rel>/<file>` entries so
    /// downstream inspection tooling can disambiguate same-basename
    /// reports across tasks (`csynth.rpt`, `csynth.xml`, …). Empty →
    /// skip the bundle step.
    pub report_paths: Vec<(Utf8PathBuf, String)>,
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

fn render_bus_ifaces(
    s_axi: &[String],
    m_axi: &[String],
    params: &[(String, Vec<(String, String)>)],
) -> String {
    let param_map: std::collections::HashMap<String, Vec<(String, String)>> = params
        .iter()
        .map(|(n, kv)| (n.clone(), kv.clone()))
        .collect();
    let mut env = minijinja::Environment::new();
    env.add_template(
        "bus_ifaces",
        include_str!("templates/bus_ifaces.tcl.j2"),
    )
    .expect("template parses");
    env.get_template("bus_ifaces")
        .expect("template exists")
        .render(minijinja::context! {
            s_axi,
            m_axi,
            m_axi_prefix => M_AXI_PREFIX,
            params => param_map,
        })
        .expect("render succeeds")
}

fn render_cpp_kernels(kernels: &[Utf8PathBuf]) -> String {
    let mut out = String::new();
    for k in kernels {
        out.push_str(" -kernel_files ");
        out.push_str(k.as_str());
    }
    out
}

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
    let mut env = minijinja::Environment::new();
    env.add_template(
        "package_xo",
        include_str!("templates/package_xo.tcl.j2"),
    )
    .expect("template parses");
    env.get_template("package_xo")
        .expect("template exists")
        .render(minijinja::context! {
            top_name,
            bus_ifaces,
            cpp_kernels,
            part_arg,
        })
        .expect("render succeeds")
}

/// Build the `.xo` for the given inputs using the provided runner.
///
/// Implements + the implementation:
///
/// 1. Allocate a staging tempdir and emit `kernel.xml` into it.
/// 2. Format `PACKAGE_XO_TCL` with the kernel's `bus_ifaces`, `cpp_kernels`,
///    and `-part` argument, and invoke Vivado via [`run_vivado`].
/// 3. Require that Vivado has produced the `.xo` at `kernel_out_path`.
/// 4. Run [`redact_xo`] on the output so two invocations on the same
///    inputs are byte-equal.
///
/// `tclargs` to Vivado: `$tmpdir $hdl_dir $xo_file $kernel_xml_path`.
pub fn pack_xo(runner: &dyn ToolRunner, inputs: &PackageXoInputs) -> Result<Utf8PathBuf> {
    let out = pack_xo_without_redaction(runner, inputs)?;
    // compatibility: bundle the HLS report files (`self.report_paths`
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
/// name, matching `PackageXo.__init__` bundling step. Any
/// existing archive entry with the same name is overwritten so callers
/// can use task-relative names (e.g. `report/<task>/csynth.xml`)
/// without colliding with the basename layout the raw `.xo` already
/// carries.
fn bundle_report_paths_into_xo(
    xo: &camino::Utf8Path,
    report_paths: &[(Utf8PathBuf, String)],
) -> Result<()> {
    use std::io::{Read, Write};
    if report_paths.is_empty() {
        return Ok(());
    }
    let raw = std::fs::read(xo)?;
    let mut z_in = zip::ZipArchive::new(std::io::Cursor::new(raw))
        .map_err(|e| XilinxError::XoRedaction(format!("open xo for bundling: {e}")))?;
    let tmp =
        tempfile::NamedTempFile::new_in(xo.parent().unwrap_or_else(|| camino::Utf8Path::new(".")).as_std_path())?;
    let written: std::collections::HashSet<&str> =
        report_paths.iter().map(|(_, name)| name.as_str()).collect();
    {
        let mut z_out = zip::ZipWriter::new(tmp.reopen()?);
        let dir_opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o755);
        for i in 0..z_in.len() {
            let mut entry = z_in
                .by_index(i)
                .map_err(|e| XilinxError::XoRedaction(format!("read xo entry {i}: {e}")))?;
            let name = entry.name().to_owned();
            if written.contains(name.as_str()) {
                continue;
            }
            if name.ends_with('/') {
                z_out
                    .add_directory(name, dir_opts)
                    .map_err(|e| XilinxError::XoRedaction(format!("copy directory entry: {e}")))?;
                continue;
            }
            let file_opts = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(entry.unix_mode().unwrap_or(0o644) & 0o777);
            z_out
                .start_file(name, file_opts)
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
                .start_file(
                    name.clone(),
                    SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated)
                        .unix_permissions(0o644),
                )
                .map_err(|e| XilinxError::XoRedaction(format!("bundle entry: {e}")))?;
            z_out.write_all(&std::fs::read(rpt)?)?;
        }
        z_out
            .finish()
            .map_err(|e| XilinxError::XoRedaction(format!("finish bundled xo: {e}")))?;
    }
    tmp.persist(xo)
        .map_err(|e| XilinxError::XoRedaction(format!("persist bundled xo: {e}")))?;
    Ok(())
}

/// Same as [`pack_xo`] but returns the raw Vivado-produced `.xo`
/// without running the reproducibility redaction pass. Primarily
/// useful for tests that need a pre-redaction artifact to exercise
/// [`redact_xo`] directly.
pub fn pack_xo_without_redaction(
    runner: &dyn ToolRunner,
    inputs: &PackageXoInputs,
) -> Result<Utf8PathBuf> {
    if !inputs.hdl_dir.is_dir() {
        return Err(XilinxError::KernelXml(format!(
            "pack_xo hdl_dir does not exist: {}",
            inputs.hdl_dir.as_str()
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
        Utf8PathBuf::from_path_buf(std::env::current_dir()?.join(&inputs.kernel_out_path))
            .unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned()))
    };
    let tmp = tempfile::tempdir()?;
    let kernel_xml_path = Utf8PathBuf::from_path_buf(tmp.path().join("kernel.xml"))
        .unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned()));
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
    let tmp_path = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf())
        .unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned()));
    let tclargs = [
        tmp_path.as_str().to_string(),
        inputs.hdl_dir.as_str().to_string(),
        kernel_out_path.as_str().to_string(),
        kernel_xml_path.as_str().to_string(),
    ];

    let mut job = VivadoJob::new(tcl);
    job.work_dir = Some(tmp_path.clone());
    job.uploads = vec![
        inputs.hdl_dir.clone(),
        tmp_path,
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
            kernel_out_path.as_str()
        )));
    }
    Ok(kernel_out_path)
}

fn redact_rpt(text: &str) -> String {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new("Date:           ... ... .. ..:..:.. ....")
            .expect("static regex compiles")
    });
    let redacted = re.replace_all(text, "Date:           Tue Jan 01 00:00:00 1980");
    redact_cpp_paths(&redacted)
}

fn redact_xml_payload(text: &str) -> String {
    match redact_xml_event_based(text) {
        Ok(out) => out,
        Err(_) => redact_cpp_paths(text),
    }
}

fn redact_xml_event_based(text: &str) -> std::result::Result<String, quick_xml::Error> {
    use quick_xml::events::{Event, BytesText};
    use quick_xml::{Reader, Writer};

    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);

    let mut writer = Writer::new(Vec::new());
    let mut buf = Vec::new();
    let mut stack: Vec<String> = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(mut e)) => {
                redact_element_attrs(&mut e);
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                stack.push(name);
                writer.write_event(Event::Start(e))?;
            }
            Ok(Event::Empty(mut e)) => {
                redact_element_attrs(&mut e);
                writer.write_event(Event::Empty(e))?;
            }
            Ok(Event::Text(t)) => {
                let text_content = t.unescape()?.into_owned();
                let redacted = if matches!(
                    stack.last().map(String::as_str),
                    Some("xilinx:coreCreationDateTime") | Some("coreCreationDateTime")
                ) {
                    "1980-01-01T00:00:00Z".to_string()
                } else {
                    redact_cpp_paths(&text_content)
                };
                writer.write_event(Event::Text(BytesText::new(&redacted)))?;
            }
            Ok(Event::End(e)) => {
                stack.pop();
                writer.write_event(Event::End(e))?;
            }
            Ok(event) => {
                writer.write_event(event)?;
            }
            Err(e) => return Err(e),
        }
        buf.clear();
    }

    Ok(String::from_utf8(writer.into_inner()).unwrap_or_default())
}

fn redact_element_attrs(elem: &mut BytesStart<'_>) {
    let attrs: Vec<(String, String)> = elem
        .attributes()
        .filter_map(|a| a.ok())
        .map(|attr| {
            let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("").to_string();
            let value = attr
                .unescape_value()
                .unwrap_or_else(|_| std::str::from_utf8(&attr.value).unwrap_or("").into())
                .into_owned();
            (key, value)
        })
        .collect();
    elem.clear_attributes();
    for (key, value) in attrs {
        let new_value = if key == "ProjectID" || key.ends_with(":ProjectID") {
            String::from("0123456789abcdef0123456789abcdef")
        } else {
            redact_source_location(&value)
        };
        elem.push_attribute((key.as_str(), new_value.as_str()));
    }
}

fn redact_source_location(text: &str) -> String {
    for marker in ["rootfscpp/", "cpp/"] {
        if let Some(idx) = text.rfind(marker) {
            return text[idx..].to_string();
        }
    }
    text.to_string()
}

fn redact_cpp_paths(text: &str) -> String {
    use std::sync::OnceLock;
    static RE_CPP_PATH: OnceLock<regex::Regex> = OnceLock::new();
    let re_cpp_path = RE_CPP_PATH.get_or_init(|| {
        regex::Regex::new(r#"(?:\.\./|/)?(?:[^\s<>"|]*/)+((?:cpp|rootfscpp)/)"#)
            .expect("static regex compiles")
    });
    re_cpp_path.replace_all(text, "$1").into_owned()
}

/// Rewrite a `.xo` ZIP in place so two invocations on the same inputs
/// produce semantically-equal outputs.
///
///   - ZIP timestamps are zeroed to the MS-DOS epoch.
///   - `*.rpt` `Date:` lines are rewritten to the epoch.
///   - `*.xml` entries have `xilinx:coreCreationDateTime`,
///     `SourceLocation` absolute paths, and `ProjectID` redacted.
///
/// Idempotent.
pub fn redact_xo(path: &camino::Utf8Path) -> Result<()> {
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
            let is_dir = name.ends_with('/');
            let unix_mode = if is_dir {
                0o755
            } else {
                entry.unix_mode().unwrap_or(0o644) & 0o777
            };
            let opts = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .last_modified_time(zip::DateTime::default())
                .unix_permissions(unix_mode);
            if is_dir {
                writer
                    .add_directory(name.clone(), opts)
                    .map_err(|e| XilinxError::XoRedaction(format!("directory: {e}")))?;
                continue;
            }
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

    fn minimal_inputs(hdl_dir: Utf8PathBuf, kernel_out_path: Utf8PathBuf) -> PackageXoInputs {
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
        let hdl_dir = Utf8PathBuf::from_path_buf(tmp.path().join("hdl")).unwrap();
        std::fs::create_dir_all(&hdl_dir).unwrap();
        std::fs::write(hdl_dir.join("top.v"), b"// stub\n").unwrap();
        // Scope current-dir into the tmp so a relative output still
        // lands in a writable place.
        let orig_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        // Stage a minimal .xo the mock runner will "produce".
        let staged = Utf8PathBuf::from_path_buf(tmp.path().join("__staged.xo")).unwrap();
        write_xo(&staged, &[("stub.txt", "ok")]);
        let staged_bytes = std::fs::read(&staged).unwrap();

        let runner = MockToolRunner::new();
        runner.push_ok("vivado", ToolOutput::default());
        let expected_abs = Utf8PathBuf::from_path_buf(tmp.path().canonicalize().unwrap().join("out.xo")).unwrap();
        runner.attach_download(&expected_abs, staged_bytes);

        let inputs = minimal_inputs(hdl_dir, Utf8PathBuf::from("out.xo"));
        let out = pack_xo(&runner, &inputs).unwrap();

        std::env::set_current_dir(orig_cwd).unwrap();
        assert!(
            out.is_absolute(),
            "pack_xo must return an absolute path; got `{}`",
            out.as_str(),
        );
        // The Vivado invocation must have received the absolute form.
        let call = &runner.calls()[0];
        let arg = call
            .args
            .iter()
            .find(|a| a.ends_with("out.xo"))
            .expect("tclargs must mention out.xo");
        assert!(
            camino::Utf8Path::new(arg).is_absolute(),
            "tclargs .xo path must be absolute; got `{arg}`",
        );
    }

    #[test]
    fn missing_hdl_dir_is_rejected() {
        let runner = MockToolRunner::new();
        let inputs = minimal_inputs(
            Utf8PathBuf::from("/nonexistent/tapa-pack-xo-hdl"),
            Utf8PathBuf::from("/tmp/k.xo"),
        );
        let err = pack_xo(&runner, &inputs).unwrap_err();
        assert!(matches!(err, XilinxError::KernelXml(_)));
    }

    #[test]
    fn pack_xo_drives_vivado_and_redacts() {
        use crate::runtime::process::ToolOutput;
        let tmp = tempfile::tempdir().unwrap();
        let hdl_dir = Utf8PathBuf::from_path_buf(tmp.path().join("hdl")).unwrap();
        std::fs::create_dir_all(&hdl_dir).unwrap();
        std::fs::write(hdl_dir.join("top.v"), b"// stub RTL\n").unwrap();
        let xo_path = Utf8PathBuf::from_path_buf(tmp.path().join("k.xo")).unwrap();

        // Stage the synthetic .xo we expect Vivado to produce (pre-redaction).
        let staged = Utf8PathBuf::from_path_buf(tmp.path().join("staged.xo")).unwrap();
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
            .any(|a| a == xo_path.as_str()));
        let mut z =
            zip::ZipArchive::new(std::io::Cursor::new(std::fs::read(&xo_path).unwrap())).unwrap();
        let mut body = String::new();
        z.by_name("ip/meta.xml")
            .unwrap()
            .read_to_string(&mut body)
            .unwrap();
        assert!(body.contains("1980-01-01T00:00:00Z"));
    }

    #[test]
    fn redacts_sandbox_cpp_paths_from_reports() {
        let left =
            "../home/.cache/bazel/_bazel_1000/sandbox/processwrapper-sandbox/62/execroot/_main/\
             bazel-out/k8-fastbuild/bin/tests/functional/reproducibility/vadd-xo.tapa/cpp/VecAdd.cpp:31";
        let right =
            "../home/.cache/bazel/_bazel_1000/sandbox/processwrapper-sandbox/54/execroot/_main/\
             bazel-out/k8-fastbuild/bin/tests/apps/vadd/vadd-xo.tapa/cpp/VecAdd.cpp:31";

        assert_eq!(
            redact_rpt(&format!("| interface | s_axilite | {left} in vecadd |")),
            redact_rpt(&format!("| interface | s_axilite | {right} in vecadd |")),
        );
        let xml =
            redact_xml_payload(&format!(r#"<Pragma location="{left}" SOURCE="{left}"/>"#));
        assert!(xml.contains(r#"location="cpp/VecAdd.cpp:31""#));
        assert!(xml.contains(r#"SOURCE="cpp/VecAdd.cpp:31""#));
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

    fn write_xo(path: &camino::Utf8Path, entries: &[(&str, &str)]) {
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
        let path = Utf8PathBuf::from_path_buf(tmp.path().join("k.xo")).unwrap();
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
    fn redaction_rewrites_remote_rootfscpp_paths() {
        let input = "\
<SourceLocation>/tmp/tapa-remote/tapa-1-2-0/rootfscpp/Add.cpp:15</SourceLocation>\n\
| /tmp/tapa-remote/tapa-1-2-0/rootfscpp/Mmap2Stream.cpp:27:20 |\n";
        let xml = redact_xml_payload(input);
        assert!(xml.contains("<SourceLocation>rootfscpp/Add.cpp:15</SourceLocation>"));
        assert!(xml.contains("| rootfscpp/Mmap2Stream.cpp:27:20 |"));
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
        let path = Utf8PathBuf::from_path_buf(tmp.path().join("k.xo")).unwrap();
        write_xo(
            &path,
            &[(
                "ip/meta.xml",
                "<xilinx:coreCreationDateTime>2024-05-17T09:15:30Z</xilinx:coreCreationDateTime>",
            )],
        );
        redact_xo(&path).unwrap();
        let mut z =
            zip::ZipArchive::new(std::io::Cursor::new(std::fs::read(&path).unwrap())).unwrap();
        let mut body = String::new();
        z.by_name("ip/meta.xml")
            .unwrap()
            .read_to_string(&mut body)
            .unwrap();
        assert!(body.contains("1980-01-01T00:00:00Z"));
    }

    #[test]
    fn redact_xo_preserves_directory_entry_modes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(tmp.path().join("k.xo")).unwrap();
        let f = std::fs::File::create(&path).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        zw.add_directory(
            "ip_repo/tapa_xrtl_Cannon_1_0/src/",
            SimpleFileOptions::default().unix_permissions(0o755),
        )
        .unwrap();
        zw.start_file(
            "ip_repo/tapa_xrtl_Cannon_1_0/src/Cannon.v",
            SimpleFileOptions::default().unix_permissions(0o644),
        )
        .unwrap();
        zw.write_all(b"module Cannon; endmodule\n").unwrap();
        zw.finish().unwrap();

        redact_xo(&path).unwrap();

        let mut z =
            zip::ZipArchive::new(std::io::Cursor::new(std::fs::read(&path).unwrap())).unwrap();
        let dir = z.by_name("ip_repo/tapa_xrtl_Cannon_1_0/src/").unwrap();
        let mode = dir.unix_mode().unwrap_or_default();
        assert_ne!(
            mode & 0o170000,
            0o100000,
            "directory entry was rewritten as a regular file: {mode:o}",
        );
        assert_eq!(mode & 0o777, 0o755);
    }
}
