//! Vitis `.ini` connectivity parser adapter.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use tapa_ir::{MemoryBank, MemoryBinding, MemoryBindings, MemoryEndpoint, MemoryKind};

use crate::platform::kernel_xml::KernelXmlPort;
use crate::SlashPlatform;

/// Errors produced while extracting memory assignments from a connectivity
/// configuration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnectivityParseError {
    #[error("line {line}: malformed section header `{text}`")]
    MalformedSection { line: usize, text: String },

    #[error("line {line}: malformed connectivity entry `{text}`; expected `key=value`")]
    MalformedEntry { line: usize, text: String },

    #[error(
        "line {line}: malformed `sp` binding `{value}`; expected `kernel.port:HBM[n]` or `kernel.port:DDR[n]`"
    )]
    MalformedBinding { line: usize, value: String },

    #[error("line {line}: malformed memory endpoint `{endpoint}`; expected `kernel.port`")]
    MalformedEndpoint { line: usize, endpoint: String },

    #[error("line {line}: unknown memory target `{target}`; expected `HBM[n]` or `DDR[n]`")]
    UnknownTarget { line: usize, target: String },

    #[error(
        "line {line}: conflicting binding for `{endpoint}`: line {first_line} assigns {first_bank}, but this entry assigns {second_bank}"
    )]
    ConflictingBinding {
        line: usize,
        endpoint: MemoryEndpoint,
        first_line: usize,
        first_bank: MemoryBank,
        second_bank: MemoryBank,
    },
}

/// One non-blank line of a Vitis `.ini` after comment stripping, with its
/// 1-based line number. [`scan_ini`] is the single strip-comment /
/// `[section]` / `key=value` pass shared by the Vitis connectivity parser
/// and the SLASH wrappers; each consumer applies its own section
/// acceptance and error rules to the items.
#[derive(Clone, Copy)]
enum IniLine<'a> {
    /// Starts with `[` — a section-header candidate.
    Section(&'a str),
    /// Contains `=` — the raw halves around the first `=`.
    Entry {
        text: &'a str,
        key: &'a str,
        value: &'a str,
    },
    /// Anything else.
    Malformed(&'a str),
}

impl<'a> IniLine<'a> {
    fn text(self) -> &'a str {
        match self {
            Self::Section(text) | Self::Malformed(text) | Self::Entry { text, .. } => text,
        }
    }
}

fn scan_ini(input: &str) -> impl Iterator<Item = (usize, IniLine<'_>)> {
    input.lines().enumerate().filter_map(|(index, raw)| {
        let text = strip_comment(raw).trim();
        if text.is_empty() {
            return None;
        }
        let item = if text.starts_with('[') {
            IniLine::Section(text)
        } else if let Some((key, value)) = text.split_once('=') {
            IniLine::Entry { text, key, value }
        } else {
            IniLine::Malformed(text)
        };
        Some((index + 1, item))
    })
}

/// The Vitis config `v++ --link` reads for a connectivity file TAPA accepts.
/// TAPA names a `tapa::mmaps` channel by its index, `sp=Top.chan[0]:HBM[0]`,
/// the spelling the floorplanner matches; `v++` knows that channel only as
/// the kernel argument `chan_0`. Each such `sp` endpoint is respelled; every
/// other line is kept verbatim.
#[must_use]
pub fn vitis_link_config(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut in_connectivity = false;
    for line in input.split_inclusive('\n') {
        let text = strip_comment(line).trim();
        if text.starts_with('[') {
            in_connectivity = text
                .strip_prefix('[')
                .and_then(|section| section.strip_suffix(']'))
                .is_some_and(|section| section.trim() == "connectivity");
        } else if let Some((kernel, port, target)) = in_connectivity
            .then(|| sp_endpoint(text))
            .flatten()
            .filter(|(_, port, _)| is_port_name(port) && port.contains('['))
        {
            let ending = &line[line.trim_end_matches(['\r', '\n']).len()..];
            let port = tapa_ir::port::sanitize_array_name(port);
            write!(output, "sp={kernel}.{port}:{target}{ending}").expect("write String");
            continue;
        }
        output.push_str(line);
    }
    output
}

/// The `(kernel, port, target)` of an `sp=kernel.port:target` entry.
fn sp_endpoint(text: &str) -> Option<(&str, &str, &str)> {
    let (key, value) = text.split_once('=')?;
    if key.trim() != "sp" {
        return None;
    }
    let (endpoint, target) = value.trim().split_once(':')?;
    let (kernel, port) = endpoint.trim().split_once('.')?;
    Some((kernel.trim(), port.trim(), target.trim()))
}

/// Parse memory assignments from a Vitis configuration string.
pub fn parse_vitis_config(input: &str) -> Result<MemoryBindings, ConnectivityParseError> {
    let mut in_connectivity = false;
    let mut bindings = Vec::<MemoryBinding>::new();
    let mut endpoint_indices = BTreeMap::<MemoryEndpoint, (usize, usize)>::new();

    for (line_number, item) in scan_ini(input) {
        let text = item.text();
        // A line ending in `]` outside `[connectivity]` (e.g.
        // `connectivity]`) is a malformed section header, not an entry;
        // `sp=k.p:HBM[0]` values only end in `]` inside the section.
        if matches!(item, IniLine::Section(_)) || (!in_connectivity && text.ends_with(']')) {
            in_connectivity = parse_section(text, line_number)? == "connectivity";
            continue;
        }
        if !in_connectivity {
            continue;
        }
        let IniLine::Entry { key, value, .. } = item else {
            return Err(ConnectivityParseError::MalformedEntry {
                line: line_number,
                text: text.to_string(),
            });
        };
        if key.trim() != "sp" {
            continue;
        }

        let binding = parse_binding(value.trim(), line_number)?;
        if let Some(&(binding_index, first_line)) = endpoint_indices.get(&binding.endpoint) {
            let first_binding = &bindings[binding_index];
            if first_binding.bank != binding.bank {
                return Err(ConnectivityParseError::ConflictingBinding {
                    line: line_number,
                    endpoint: binding.endpoint,
                    first_line,
                    first_bank: first_binding.bank,
                    second_bank: binding.bank,
                });
            }
            continue;
        }

        endpoint_indices.insert(binding.endpoint.clone(), (bindings.len(), line_number));
        bindings.push(binding);
    }

    Ok(MemoryBindings::try_from_bindings(bindings).expect("parser deduplicates memory endpoints"))
}

/// Resolve a single-kernel SLASH configuration to TAPA's logical top ports.
/// Physical interface names come from the typed compiled ports; array names
/// are never reconstructed by guessing which underscores used to be brackets.
pub fn parse_slash_config(
    input: &str,
    top: &str,
    ports: &[KernelXmlPort],
    platform: &SlashPlatform,
) -> Result<MemoryBindings, String> {
    slash_bindings(&parse_slash(input, top, platform)?, top, ports, platform)
}

/// Validate a SLASH configuration and add the one kernel clock when absent.
/// Existing clock sections must all name the sole `nk` instance at exactly the
/// requested frequency. Returns the resolved bindings alongside the prepared
/// text so callers need not parse the same config twice.
pub fn prepare_slash_config(
    input: &str,
    top: &str,
    ports: &[KernelXmlPort],
    requested_hz: u64,
    platform: &SlashPlatform,
) -> Result<(MemoryBindings, String), String> {
    let parsed = parse_slash(input, top, platform)?;
    let bindings = slash_bindings(&parsed, top, ports, platform)?;
    let mut config = enforce_slash_clock(input, &parsed, requested_hz)?;
    if !parsed.has_shell && platform.shell != "service" {
        insert_shell_line(input, &mut config, platform.shell);
    }
    Ok((bindings, config))
}

/// The SLASH-relevant slices of one config text, collected in a single
/// scan: the `nk` instance, the `shell` flag, the trimmed `sp` values of
/// `[connectivity]`, and the stripped lines of every `[clock]` section.
struct SlashConfig<'a> {
    instance: &'a str,
    entries: Vec<&'a str>,
    clocks: Vec<Vec<&'a str>>,
    has_shell: bool,
}

fn parse_slash<'a>(
    input: &'a str,
    top: &str,
    platform: &SlashPlatform,
) -> Result<SlashConfig<'a>, String> {
    let mut in_connectivity = false;
    let mut in_clock = false;
    let mut clock_lines: Vec<&'a str> = Vec::new();
    let mut clocks = Vec::new();
    let mut instance = None;
    let mut entries = Vec::new();
    let mut has_shell = false;
    for (_, item) in scan_ini(input) {
        let text = item.text();
        match item {
            IniLine::Section(_) => {
                if in_clock {
                    clocks.push(std::mem::take(&mut clock_lines));
                }
                in_connectivity = text == "[connectivity]";
                in_clock = text.eq_ignore_ascii_case("[clock]");
            }
            IniLine::Malformed(_) | IniLine::Entry { .. } if in_clock => clock_lines.push(text),
            IniLine::Malformed(_) if in_connectivity => {
                return Err(format!("invalid SLASH entry `{text}`"));
            }
            IniLine::Entry { key, value, .. } if in_connectivity => match key.trim() {
                "shell" => {
                    if has_shell || value.trim() != platform.shell {
                        return Err(format!(
                            "expected at most one `shell={}` for `{}`",
                            platform.shell, platform.name
                        ));
                    }
                    has_shell = true;
                }
                "nk" => {
                    let fields: Vec<_> = value.trim().split(':').collect();
                    if instance.is_some()
                        || fields.len() != 3
                        || fields[0] != top
                        || fields[1] != "1"
                        || !is_identifier(fields[2])
                    {
                        return Err(format!("expected one SLASH `nk={top}:1:<instance>`"));
                    }
                    instance = Some(fields[2]);
                }
                "sp" => entries.push(value.trim()),
                _ => {}
            },
            _ => {}
        }
    }
    if in_clock {
        clocks.push(clock_lines);
    }
    let instance = instance.ok_or_else(|| format!("missing SLASH `nk={top}:1:<instance>`"))?;
    Ok(SlashConfig {
        instance,
        entries,
        clocks,
        has_shell,
    })
}

/// Resolve the parsed `sp` entries against the typed compiled ports into
/// memory bindings on the logical top ports.
fn slash_bindings(
    parsed: &SlashConfig<'_>,
    top: &str,
    ports: &[KernelXmlPort],
    platform: &SlashPlatform,
) -> Result<MemoryBindings, String> {
    let mut normalized = String::from("[connectivity]\n");
    for entry in &parsed.entries {
        let (endpoint, target) = entry
            .split_once(':')
            .ok_or_else(|| format!("invalid SLASH sp `{entry}`"))?;
        let physical = endpoint
            .trim()
            .strip_prefix(&format!("{}.", parsed.instance))
            .ok_or_else(|| format!("unknown kernel instance in `{entry}`"))?;
        let logical = ports
            .iter()
            .find(|port| port.compiled_m_axi_name().as_deref() == Some(physical))
            .map(|port| &port.name)
            .ok_or_else(|| format!("SLASH port `{physical}` is not a compiled top interface"))?;
        let (kind, index) = target
            .trim()
            .strip_prefix("HBM")
            .map(|v| ("HBM", v))
            .or_else(|| target.trim().strip_prefix("DDR").map(|v| ("DDR", v)))
            .ok_or_else(|| format!("unsupported SLASH memory `{target}`"))?;
        writeln!(normalized, "sp={top}.{logical}:{kind}[{index}]").expect("write String");
    }
    let bindings = parse_vitis_config(&normalized).map_err(|error| error.to_string())?;
    for binding in bindings.iter() {
        let limit = match binding.bank.kind {
            MemoryKind::Hbm => platform.hbm_ports,
            MemoryKind::Ddr => platform.ddr_ports,
        };
        if binding.bank.index >= limit {
            return Err(format!(
                "{} is not available on {}",
                binding.bank, platform.name
            ));
        }
    }
    Ok(bindings)
}

/// Insert `shell=<shell>` directly under the `[connectivity]` header. The
/// line offsets of `input` index into `output` as well, whose only edit so
/// far is an appended `[clock]` section.
fn insert_shell_line(input: &str, output: &mut String, shell: &str) {
    let mut offset = 0;
    for line in input.split_inclusive('\n') {
        if strip_comment(line).trim() == "[connectivity]" {
            output.insert_str(
                offset + line.trim_end_matches(['\r', '\n']).len(),
                &format!("\nshell={shell}"),
            );
            return;
        }
        offset += line.len();
    }
}

/// Validate every `[clock]` section against the one kernel instance and
/// append the requested clock when the config has no `[clock]` section.
fn enforce_slash_clock(
    input: &str,
    parsed: &SlashConfig<'_>,
    requested_hz: u64,
) -> Result<String, String> {
    if requested_hz == 0 {
        return Err("SLASH clock must be positive".to_string());
    }
    for clock_lines in &parsed.clocks {
        let mut kernel = None;
        let mut frequency = None;
        for line in clock_lines {
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("invalid SLASH [clock] entry `{line}`"));
            };
            match key.trim().to_ascii_lowercase().as_str() {
                "krnl" => kernel = Some(value.trim()),
                "freqhz" => frequency = Some(value.trim()),
                _ => {}
            }
        }
        validate_clock_section(kernel, frequency, parsed.instance, requested_hz)?;
    }

    let mut output = input.to_string();
    if parsed.clocks.is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        if !output.is_empty() {
            output.push('\n');
        }
        writeln!(
            output,
            "[clock]\nkrnl={}\nfreqhz={requested_hz}",
            parsed.instance
        )
        .expect("write String");
    }
    Ok(output)
}

fn validate_clock_section(
    kernel: Option<&str>,
    frequency: Option<&str>,
    instance: &str,
    requested_hz: u64,
) -> Result<(), String> {
    let actual_kernel =
        kernel.ok_or_else(|| "incomplete SLASH [clock]: missing `krnl`".to_string())?;
    let actual_frequency = frequency
        .ok_or_else(|| "incomplete SLASH [clock]: missing `freqhz`".to_string())?
        .parse::<u64>()
        .map_err(|_| "invalid SLASH [clock] `freqhz`".to_string())?;
    if actual_kernel != instance || actual_frequency != requested_hz {
        return Err(format!(
            "SLASH [clock] must be `krnl={instance}` and `freqhz={requested_hz}`, \
             found `krnl={actual_kernel}` and `freqhz={actual_frequency}`"
        ));
    }
    Ok(())
}

fn strip_comment(line: &str) -> &str {
    let comment_start = line
        .char_indices()
        .filter_map(|(index, character)| matches!(character, '#' | ';').then_some(index))
        .min();
    comment_start.map_or(line, |index| &line[..index])
}

fn parse_section(line: &str, line_number: usize) -> Result<&str, ConnectivityParseError> {
    let Some(section) = line
        .strip_prefix('[')
        .and_then(|line| line.strip_suffix(']'))
    else {
        return Err(ConnectivityParseError::MalformedSection {
            line: line_number,
            text: line.to_string(),
        });
    };
    let section = section.trim();
    if section.is_empty() || section.contains(['[', ']']) {
        return Err(ConnectivityParseError::MalformedSection {
            line: line_number,
            text: line.to_string(),
        });
    }
    Ok(section)
}

fn parse_binding(value: &str, line: usize) -> Result<MemoryBinding, ConnectivityParseError> {
    let Some((endpoint, target)) = value.split_once(':') else {
        return Err(ConnectivityParseError::MalformedBinding {
            line,
            value: value.to_string(),
        });
    };
    if target.contains("]:") {
        return Err(ConnectivityParseError::MalformedBinding {
            line,
            value: value.to_string(),
        });
    }

    Ok(MemoryBinding {
        endpoint: parse_endpoint(endpoint.trim(), line)?,
        bank: parse_bank(target.trim(), line)?,
    })
}

fn parse_endpoint(value: &str, line: usize) -> Result<MemoryEndpoint, ConnectivityParseError> {
    let mut parts = value.split('.');
    let kernel = parts.next().unwrap_or_default().trim();
    let port = parts.next().unwrap_or_default().trim();
    if parts.next().is_some() || !is_identifier(kernel) || !is_port_name(port) {
        return Err(ConnectivityParseError::MalformedEndpoint {
            line,
            endpoint: value.to_string(),
        });
    }
    Ok(MemoryEndpoint {
        kernel: kernel.to_string(),
        port: port.to_string(),
    })
}

fn is_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

/// Like [`is_identifier`] but also allows one or more `[N]` array-index
/// suffixes, e.g. `edge_list_ch[0]`. The expanded ports of a `tapa::mmaps<T, N>`
/// argument are named with brackets, and the connectivity file must reference
/// them by that exact name (just as the memory bank uses `HBM[0]`).
fn is_port_name(value: &str) -> bool {
    let Some(base_end) = value.find('[') else {
        return is_identifier(value);
    };
    let base = &value[..base_end];
    if base.is_empty() || !is_identifier(base) {
        return false;
    }
    let mut rest = &value[base_end..];
    let mut saw_index = false;
    while let Some(inner) = rest.strip_prefix('[') {
        match inner.split_once(']') {
            Some((digits, after))
                if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                rest = after;
                saw_index = true;
            }
            _ => return false,
        }
    }
    saw_index && rest.is_empty()
}

fn parse_bank(value: &str, line: usize) -> Result<MemoryBank, ConnectivityParseError> {
    value
        .parse()
        .map_err(|_| ConnectivityParseError::UnknownTarget {
            line,
            target: value.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tapa_ir::MemoryKind;

    #[test]
    fn vitis_link_config_respells_only_indexed_connectivity_endpoints() {
        let input = "# kept\n[connectivity]\nnk=Top:1:Top\n\
                     sp = Top.chan[0] : HBM[0] ; channel\r\nsp=Top.a:DDR[1]\n\
                     [vivado]\nprop=run.impl_1.strategy=Explore\n[connectivity]\nsp=Top.chan[3]:HBM[3]";
        assert_eq!(
            vitis_link_config(input),
            "# kept\n[connectivity]\nnk=Top:1:Top\n\
             sp=Top.chan_0:HBM[0]\r\nsp=Top.a:DDR[1]\n\
             [vivado]\nprop=run.impl_1.strategy=Explore\n[connectivity]\nsp=Top.chan_3:HBM[3]"
        );
        assert_eq!(
            parse_vitis_config(input).expect("valid").len(),
            3,
            "the respelled file describes the same bindings"
        );
    }

    fn slash_port(name: &str) -> KernelXmlPort {
        KernelXmlPort {
            name: name.to_string(),
            category: crate::PortCategory::MAxi,
            width: 512,
            port: String::new(),
            ctype: "int*".to_string(),
        }
    }

    fn service() -> &'static SlashPlatform {
        crate::find_slash_platform("slash_v80_service").expect("service")
    }

    #[test]
    fn slash_shell_selection_and_memory_limits_are_checked_without_floorplanning() {
        let compute = crate::find_slash_platform("slash_v80_compute").expect("compute");
        for (platform, target) in [(service(), "DDR3"), (compute, "DDR11"), (compute, "HBM63")] {
            let input = format!(
                "[connectivity] # shell selection\nnk=Top:1:Top_0\nsp=Top_0.m_axi_a:{target}\n"
            );
            let (_, prepared) =
                prepare_slash_config(&input, "Top", &[slash_port("a")], 200_000_000, platform)
                    .expect("available interface");
            assert_eq!(
                prepared.contains("shell=compute"),
                platform.shell == "compute"
            );
            parse_slash_config(&prepared, "Top", &[slash_port("a")], platform)
                .expect("prepared config");
        }
        for (platform, entry) in [
            (service(), "sp=Top_0.m_axi_a:DDR4"),
            (compute, "sp=Top_0.m_axi_a:DDR12"),
            (compute, "sp=Top_0.m_axi_a:HBM64"),
            (compute, "shell=service"),
            (service(), "shell=compute"),
            (compute, "shell=compute\nshell=compute"),
            (compute, "shell=unknown"),
        ] {
            let input = format!("[connectivity]\nnk=Top:1:Top_0\n{entry}\n");
            parse_slash_config(&input, "Top", &[slash_port("a")], platform)
                .expect_err("wrong shell or unavailable interface");
        }
    }

    #[test]
    fn slash_config_uses_typed_compiled_ports_and_appends_clock() {
        let input = "[connectivity]\nnk=VecAdd:1:VecAdd_0\nsp=VecAdd_0.m_axi_a:HBM0\n";
        let (_, prepared) =
            prepare_slash_config(input, "VecAdd", &[slash_port("a")], 300_000_000, service())
                .expect("valid SLASH config");
        assert!(prepared.ends_with("[clock]\nkrnl=VecAdd_0\nfreqhz=300000000\n"));
        let bindings = parse_slash_config(&prepared, "VecAdd", &[slash_port("a")], service())
            .expect("typed port resolves");
        assert_eq!(
            bindings.get("VecAdd", "a"),
            Some(MemoryBank {
                kind: MemoryKind::Hbm,
                index: 0,
            })
        );
    }

    #[test]
    fn slash_config_accepts_matching_clock_and_rejects_drift() {
        let input =
            "[connectivity]\nnk=VecAdd:1:VecAdd_0\n\n[clock]\nkrnl=VecAdd_0\nfreqhz=300000000\n";
        assert_eq!(
            prepare_slash_config(input, "VecAdd", &[slash_port("a")], 300_000_000, service())
                .expect("matching clock")
                .1,
            input,
        );
        prepare_slash_config(input, "VecAdd", &[slash_port("a")], 200_000_000, service())
            .expect_err("different requested clock must fail");
        prepare_slash_config(
            "[connectivity]\nnk=VecAdd:1:VecAdd_0\nsp=VecAdd_0.m_axi_missing:HBM0\n",
            "VecAdd",
            &[slash_port("a")],
            300_000_000,
            service(),
        )
        .expect_err("unknown compiled port must fail");
    }

    #[test]
    fn parses_whitespace_comments_sections_and_repeated_entries() {
        let config = "
            # Global comment.
            [clock]
            freqHz = 300000000:kernel.ap_clk

            [ connectivity ] ; section comment
            nk = kernel:1:kernel_1
            sp = kernel_1.input : HBM[ 3 ] # entry comment
            sp=kernel_1.output:DDR[1]
            stream_connect = kernel_1.out:other_1.in

            [advanced]
            param = compiler.userPostSysLinkOverlayTcl=overlay.tcl

            [connectivity]
            sp = other_1.table : HBM[31] ; another entry
        ";

        let bindings = parse_vitis_config(config).expect("parse connectivity");
        assert_eq!(
            bindings.len(),
            3,
            "all repeated sp entries must be retained"
        );
        assert_eq!(
            bindings.get("kernel_1", "input"),
            Some(MemoryBank {
                kind: MemoryKind::Hbm,
                index: 3,
            }),
            "HBM binding must be parsed",
        );
        assert_eq!(
            bindings.get("kernel_1", "output"),
            Some(MemoryBank {
                kind: MemoryKind::Ddr,
                index: 1,
            }),
            "DDR binding must be parsed",
        );
        assert_eq!(
            bindings
                .iter()
                .map(|binding| binding.endpoint.to_string())
                .collect::<Vec<_>>(),
            ["kernel_1.input", "kernel_1.output", "other_1.table"],
            "first-occurrence order must be stable",
        );
        assert_eq!(
            bindings.iter().nth(2).unwrap().bank.to_string(),
            "HBM[31]",
            "bank display must match configuration syntax",
        );
    }

    #[test]
    fn identical_duplicate_is_idempotent() {
        let config = "[connectivity]\nsp=k.p:HBM[0]\nsp = k.p : HBM[0]\n";
        let bindings = parse_vitis_config(config).expect("parse duplicate");
        assert_eq!(
            bindings.len(),
            1,
            "identical duplicates must be deduplicated"
        );
    }

    #[test]
    fn parses_array_index_endpoints_for_mmaps_args() {
        // A `tapa::mmaps<T, N>` argument expands to per-channel ports named
        // with `[N]` indices; the connectivity must reference them by that
        // exact bracketed name, matching the memory bank's own bracket form.
        let config =
            "[connectivity]\nsp=Sextans.edge_list_ch[0]:HBM[1]\nsp=Sextans.edge_list_ch[7]:HBM[8]\n";
        let bindings = parse_vitis_config(config).expect("array endpoints parse");
        assert_eq!(bindings.len(), 2);
        assert_eq!(
            bindings.get("Sextans", "edge_list_ch[0]"),
            Some(MemoryBank {
                kind: MemoryKind::Hbm,
                index: 1,
            }),
            "bracketed port name must be retained verbatim",
        );
        assert_eq!(
            bindings.get("Sextans", "edge_list_ch[7]"),
            Some(MemoryBank {
                kind: MemoryKind::Hbm,
                index: 8,
            }),
        );
    }

    #[test]
    fn rejects_malformed_array_index_endpoints() {
        for config in [
            "[connectivity]\nsp=kernel.port[]:HBM[0]",
            "[connectivity]\nsp=kernel.port[x]:HBM[0]",
            "[connectivity]\nsp=kernel.[0]:HBM[0]",
            "[connectivity]\nsp=kernel.port[0]extra:HBM[0]",
        ] {
            parse_vitis_config(config).expect_err("malformed array endpoint must fail");
        }
    }

    #[test]
    fn conflicting_duplicate_reports_both_assignments() {
        let config = "[connectivity]\nsp=k.p:HBM[0]\n# gap\nsp=k.p:DDR[1]\n";
        let error = parse_vitis_config(config).expect_err("conflict must fail");
        assert_eq!(
            error,
            ConnectivityParseError::ConflictingBinding {
                line: 4,
                endpoint: MemoryEndpoint {
                    kernel: "k".to_string(),
                    port: "p".to_string(),
                },
                first_line: 2,
                first_bank: MemoryBank {
                    kind: MemoryKind::Hbm,
                    index: 0,
                },
                second_bank: MemoryBank {
                    kind: MemoryKind::Ddr,
                    index: 1,
                },
            },
            "conflict diagnostics must retain source locations and banks",
        );
    }

    #[test]
    fn rejects_malformed_entries_and_endpoints() {
        let cases = [
            ("[connectivity]\nsp k.p:HBM[0]", "expected `key=value`"),
            ("[connectivity]\nsp=k.p", "expected `kernel.port:HBM[n]`"),
            ("[connectivity]\nsp=kernel:HBM[0]", "expected `kernel.port`"),
            ("[connectivity]\nsp=.port:HBM[0]", "expected `kernel.port`"),
            (
                "[connectivity]\nsp=kernel.bad-port:HBM[0]",
                "expected `kernel.port`",
            ),
            (
                "[connectivity]\nsp=kernel.port.extra:HBM[0]",
                "expected `kernel.port`",
            ),
            (
                "[connectivity]\nsp=kernel.port:HBM[0]:DDR[1]",
                "expected `kernel.port:HBM[n]`",
            ),
        ];

        for (config, expected) in cases {
            let error = parse_vitis_config(config).expect_err("malformed entry must fail");
            assert!(
                error.to_string().contains(expected),
                "unexpected error `{error}` for `{config}`",
            );
        }
    }

    #[test]
    fn rejects_malformed_section_headers() {
        for config in ["[connectivity", "connectivity]", "[[connectivity]]"] {
            let error = parse_vitis_config(config).expect_err("malformed section must fail");
            assert!(
                matches!(error, ConnectivityParseError::MalformedSection { .. }),
                "unexpected error `{error}` for `{config}`",
            );
        }
    }

    #[test]
    fn rejects_unknown_target_syntax() {
        for target in [
            "PLRAM[0]",
            "hbm[0]",
            "HBM",
            "HBM[]",
            "HBM[x]",
            "HBM[0:1]",
            "HBM[+1]",
            "HBM[4294967296]",
            "DDR[-1]",
            "DDR[1]extra",
        ] {
            let config = format!("[connectivity]\nsp=kernel.port:{target}");
            let error = parse_vitis_config(&config).expect_err("unknown target must fail");
            assert_eq!(
                error,
                ConnectivityParseError::UnknownTarget {
                    line: 2,
                    target: target.to_string(),
                },
                "unexpected target diagnostic for `{target}`",
            );
        }
    }

    #[test]
    fn returns_empty_when_no_memory_bindings_exist() {
        let config = "[connectivity]\nnk=kernel:1\nstream_connect=a.out:b.in\n";
        let bindings = parse_vitis_config(config).expect("parse unrelated connectivity");
        assert!(
            bindings.is_empty(),
            "unrelated keys must not become bindings"
        );
    }
}
