//! Vitis `.ini` connectivity parser adapter.

use std::collections::BTreeMap;

use tapa_ir::{MemoryBank, MemoryBinding, MemoryBindings, MemoryEndpoint, MemoryKind};

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
