//! Vitis `.ini` connectivity parser adapter.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use tapa_ir::{MemoryBank, MemoryBinding, MemoryBindings, MemoryEndpoint, MemoryKind};

use crate::platform::kernel_xml::KernelXmlPort;

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

/// Parse memory assignments from a Vitis configuration string.
pub fn parse_vitis_config(input: &str) -> Result<MemoryBindings, ConnectivityParseError> {
    let mut in_connectivity = false;
    let mut bindings = Vec::<MemoryBinding>::new();
    let mut endpoint_indices = BTreeMap::<MemoryEndpoint, (usize, usize)>::new();

    for (line_index, original_line) in input.lines().enumerate() {
        let line_number = line_index + 1;
        let line = strip_comment(original_line).trim();
        if line.is_empty() {
            continue;
        }

        if line.starts_with('[') || (!in_connectivity && line.ends_with(']')) {
            let section = parse_section(line, line_number)?;
            in_connectivity = section == "connectivity";
            continue;
        }

        if !in_connectivity {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            return Err(ConnectivityParseError::MalformedEntry {
                line: line_number,
                text: line.to_string(),
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
) -> Result<MemoryBindings, String> {
    parse_slash(input, top, ports).map(|parsed| parsed.bindings)
}

/// Validate a SLASH configuration and add the one kernel clock when absent.
/// Existing clock sections must all name the sole `nk` instance at exactly the
/// requested frequency.
pub fn prepare_slash_config(
    input: &str,
    top: &str,
    ports: &[KernelXmlPort],
    requested_hz: u64,
) -> Result<String, String> {
    let parsed = parse_slash(input, top, ports)?;
    enforce_slash_clock(input, &parsed.instance, requested_hz)
}

struct ParsedSlashConfig {
    instance: String,
    bindings: MemoryBindings,
}

fn parse_slash(
    input: &str,
    top: &str,
    ports: &[KernelXmlPort],
) -> Result<ParsedSlashConfig, String> {
    let mut in_connectivity = false;
    let mut instance = None;
    let mut entries = Vec::new();
    for original in input.lines() {
        let line = strip_comment(original).trim();
        if line.starts_with('[') {
            in_connectivity = line == "[connectivity]";
            continue;
        }
        if !in_connectivity || line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("invalid SLASH entry `{line}`"));
        };
        match key.trim() {
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
        }
    }
    let instance = instance.ok_or_else(|| format!("missing SLASH `nk={top}:1:<instance>`"))?;
    let mut normalized = String::from("[connectivity]\n");
    for entry in entries {
        let (endpoint, target) = entry
            .split_once(':')
            .ok_or_else(|| format!("invalid SLASH sp `{entry}`"))?;
        let physical = endpoint
            .trim()
            .strip_prefix(&format!("{instance}."))
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
    Ok(ParsedSlashConfig {
        instance: instance.to_string(),
        bindings,
    })
}

fn enforce_slash_clock(input: &str, instance: &str, requested_hz: u64) -> Result<String, String> {
    if requested_hz == 0 {
        return Err("SLASH clock must be positive".to_string());
    }
    let mut in_clock = false;
    let mut saw_clock = false;
    let mut clock_kernel: Option<String> = None;
    let mut clock_frequency: Option<String> = None;

    for original in input.lines() {
        let line = strip_comment(original).trim();
        if line.starts_with('[') {
            validate_clock_section(
                in_clock,
                clock_kernel.as_deref(),
                clock_frequency.as_deref(),
                instance,
                requested_hz,
            )?;
            in_clock = line.eq_ignore_ascii_case("[clock]");
            saw_clock |= in_clock;
            clock_kernel = None;
            clock_frequency = None;
            continue;
        }
        if !in_clock || line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("invalid SLASH [clock] entry `{line}`"));
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "krnl" => clock_kernel = Some(value.trim().to_string()),
            "freqhz" => clock_frequency = Some(value.trim().to_string()),
            _ => {}
        }
    }
    validate_clock_section(
        in_clock,
        clock_kernel.as_deref(),
        clock_frequency.as_deref(),
        instance,
        requested_hz,
    )?;

    let mut output = input.to_string();
    if !saw_clock {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        if !output.is_empty() {
            output.push('\n');
        }
        writeln!(output, "[clock]\nkrnl={instance}\nfreqhz={requested_hz}").expect("write String");
    }
    Ok(output)
}

fn validate_clock_section(
    saw_section: bool,
    kernel: Option<&str>,
    frequency: Option<&str>,
    instance: &str,
    requested_hz: u64,
) -> Result<(), String> {
    if !saw_section {
        return Ok(());
    }
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
    let (kind, index) = if let Some(index) = value
        .strip_prefix("HBM[")
        .and_then(|value| value.strip_suffix(']'))
    {
        (MemoryKind::Hbm, index)
    } else if let Some(index) = value
        .strip_prefix("DDR[")
        .and_then(|value| value.strip_suffix(']'))
    {
        (MemoryKind::Ddr, index)
    } else {
        return Err(ConnectivityParseError::UnknownTarget {
            line,
            target: value.to_string(),
        });
    };

    let index = index.trim();
    if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ConnectivityParseError::UnknownTarget {
            line,
            target: value.to_string(),
        });
    }
    let index = index
        .parse()
        .map_err(|_| ConnectivityParseError::UnknownTarget {
            line,
            target: value.to_string(),
        })?;
    Ok(MemoryBank { kind, index })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slash_port(name: &str) -> KernelXmlPort {
        KernelXmlPort {
            name: name.to_string(),
            category: crate::PortCategory::MAxi,
            width: 512,
            port: String::new(),
            ctype: "int*".to_string(),
        }
    }

    #[test]
    fn slash_config_uses_typed_compiled_ports_and_appends_clock() {
        let input = "[connectivity]\nnk=VecAdd:1:VecAdd_0\nsp=VecAdd_0.m_axi_a:HBM0\n";
        let prepared = prepare_slash_config(input, "VecAdd", &[slash_port("a")], 300_000_000)
            .expect("valid SLASH config");
        assert!(prepared.ends_with("[clock]\nkrnl=VecAdd_0\nfreqhz=300000000\n"));
        let bindings = parse_slash_config(&prepared, "VecAdd", &[slash_port("a")])
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
            prepare_slash_config(input, "VecAdd", &[slash_port("a")], 300_000_000)
                .expect("matching clock"),
            input,
        );
        prepare_slash_config(input, "VecAdd", &[slash_port("a")], 200_000_000)
            .expect_err("different requested clock must fail");
        prepare_slash_config(
            "[connectivity]\nnk=VecAdd:1:VecAdd_0\nsp=VecAdd_0.m_axi_missing:HBM0\n",
            "VecAdd",
            &[slash_port("a")],
            300_000_000,
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
            bindings.as_slice()[2].bank.to_string(),
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
