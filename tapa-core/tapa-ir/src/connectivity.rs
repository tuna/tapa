//! Plain-data model and parser for external memory connectivity.

use std::collections::BTreeMap;
use std::fmt;

/// A kernel argument named by a connectivity entry.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct MemoryEndpoint {
    pub kernel: String,
    pub port: String,
}

impl fmt::Display for MemoryEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.kernel, self.port)
    }
}

/// A kind of external memory bank supported by the connectivity parser.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Hbm,
    Ddr,
}

/// One indexed external memory bank.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct MemoryBank {
    pub kind: MemoryKind,
    pub index: u32,
}

impl fmt::Display for MemoryBank {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            MemoryKind::Hbm => "HBM",
            MemoryKind::Ddr => "DDR",
        };
        write!(f, "{kind}[{}]", self.index)
    }
}

/// A tag that is not a memory bank of the form `HBM[i]` or `DDR[i]`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not a memory bank tag of the form `HBM[i]` or `DDR[i]`")]
pub struct MemoryBankParseError(pub String);

impl std::str::FromStr for MemoryBank {
    type Err = MemoryBankParseError;

    fn from_str(tag: &str) -> Result<Self, Self::Err> {
        let (kind, rest) = tag
            .strip_prefix("HBM[")
            .map(|rest| (MemoryKind::Hbm, rest))
            .or_else(|| tag.strip_prefix("DDR[").map(|rest| (MemoryKind::Ddr, rest)))
            .ok_or_else(|| MemoryBankParseError(tag.to_string()))?;
        let index = rest
            .strip_suffix(']')
            .and_then(|digits| digits.parse::<u32>().ok())
            .ok_or_else(|| MemoryBankParseError(tag.to_string()))?;
        Ok(Self { kind, index })
    }
}

/// One kernel-port-to-memory-bank assignment.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct MemoryBinding {
    pub endpoint: MemoryEndpoint,
    pub bank: MemoryBank,
}

/// The unique memory assignments extracted from a connectivity configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct MemoryBindings {
    bindings: Vec<MemoryBinding>,
}

impl<'de> serde::Deserialize<'de> for MemoryBindings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "snake_case", deny_unknown_fields)]
        struct WireFormat {
            bindings: Vec<MemoryBinding>,
        }

        let wire = WireFormat::deserialize(deserializer)?;
        let mut endpoints = BTreeMap::<&MemoryEndpoint, MemoryBank>::new();
        for binding in &wire.bindings {
            if let Some(previous) = endpoints.insert(&binding.endpoint, binding.bank) {
                let detail = if previous == binding.bank {
                    "is repeated".to_string()
                } else {
                    format!("maps to both {previous} and {}", binding.bank)
                };
                return Err(serde::de::Error::custom(format!(
                    "memory endpoint `{}` {detail}",
                    binding.endpoint
                )));
            }
        }
        Ok(Self {
            bindings: wire.bindings,
        })
    }
}

impl MemoryBindings {
    /// Parse `sp=kernel.port:HBM[n]` and `sp=kernel.port:DDR[n]` entries from
    /// a Vitis link config. `port` may carry one or more `[N]` array-index
    /// suffixes for `tapa::mmaps<T, N>` arguments, e.g. `kernel.fifo[3]:HBM[4]`.
    /// `[connectivity]` sections in a Vitis configuration.
    pub fn parse_vitis_config(input: &str) -> Result<Self, ConnectivityParseError> {
        parse_vitis_config(input)
    }

    /// Return all bindings in their first-occurrence order.
    pub fn as_slice(&self) -> &[MemoryBinding] {
        &self.bindings
    }

    /// Iterate over all bindings in their first-occurrence order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &MemoryBinding> {
        self.bindings.iter()
    }

    /// Find the bank assigned to `kernel.port`.
    pub fn get(&self, kernel: &str, port: &str) -> Option<MemoryBank> {
        self.bindings
            .iter()
            .find(|binding| binding.endpoint.kernel == kernel && binding.endpoint.port == port)
            .map(|binding| binding.bank)
    }

    /// Return the number of unique endpoint bindings.
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    /// Return whether the configuration has no memory bindings.
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}

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

    Ok(MemoryBindings { bindings })
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

        let bindings = MemoryBindings::parse_vitis_config(config).expect("parse connectivity");
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

    #[test]
    fn serde_round_trip_has_stable_snake_case_form() {
        let bindings = parse_vitis_config(
            "[connectivity]\nsp=kernel_1.input:HBM[3]\nsp=kernel_1.output:DDR[1]\n",
        )
        .expect("parse connectivity");
        let json = serde_json::to_string(&bindings).expect("serialize bindings");
        assert_eq!(
            json,
            r#"{"bindings":[{"endpoint":{"kernel":"kernel_1","port":"input"},"bank":{"kind":"hbm","index":3}},{"endpoint":{"kernel":"kernel_1","port":"output"},"bank":{"kind":"ddr","index":1}}]}"#,
            "the serialized form is a stable public contract",
        );
        assert_eq!(
            serde_json::from_str::<MemoryBindings>(&json).expect("deserialize bindings"),
            bindings,
            "bindings must round-trip exactly",
        );
    }

    #[test]
    fn serde_rejects_unknown_fields_and_kinds() {
        let cases = [
            r#"{"bindings":[],"extra":0}"#,
            r#"{"bindings":[{"endpoint":{"kernel":"k","port":"p","extra":0},"bank":{"kind":"hbm","index":0}}]}"#,
            r#"{"bindings":[{"endpoint":{"kernel":"k","port":"p"},"bank":{"kind":"hbm","index":0,"extra":0}}]}"#,
            r#"{"bindings":[{"endpoint":{"kernel":"k","port":"p"},"bank":{"kind":"hbm","index":0},"extra":0}]}"#,
            r#"{"bindings":[{"endpoint":{"kernel":"k","port":"p"},"bank":{"kind":"plram","index":0}}]}"#,
        ];

        for json in cases {
            let error = serde_json::from_str::<MemoryBindings>(json)
                .expect_err("unknown JSON fields and kinds must fail");
            assert!(
                error.to_string().contains("unknown"),
                "unexpected error `{error}` for `{json}`",
            );
        }
    }

    #[test]
    fn serde_rejects_duplicate_endpoints() {
        for json in [
            r#"{"bindings":[{"endpoint":{"kernel":"k","port":"p"},"bank":{"kind":"hbm","index":0}},{"endpoint":{"kernel":"k","port":"p"},"bank":{"kind":"hbm","index":0}}]}"#,
            r#"{"bindings":[{"endpoint":{"kernel":"k","port":"p"},"bank":{"kind":"hbm","index":0}},{"endpoint":{"kernel":"k","port":"p"},"bank":{"kind":"ddr","index":1}}]}"#,
        ] {
            let error = serde_json::from_str::<MemoryBindings>(json)
                .expect_err("duplicate JSON endpoints must fail");
            assert!(
                error.to_string().contains("memory endpoint `k.p`"),
                "unexpected error `{error}` for `{json}`",
            );
        }
    }

    #[test]
    fn memory_bank_display_round_trips_through_fromstr() {
        for (tag, kind, index) in [
            ("HBM[0]", MemoryKind::Hbm, 0),
            ("HBM[31]", MemoryKind::Hbm, 31),
            ("DDR[3]", MemoryKind::Ddr, 3),
        ] {
            let bank: MemoryBank = tag.parse().expect("parse");
            assert_eq!((bank.kind, bank.index), (kind, index));
            assert_eq!(bank.to_string(), tag);
        }
        for bad in ["HBM", "DDR", "HBM[]", "HBM[x]", "PLRAM[0]", "hbm[1]"] {
            assert!(bad.parse::<MemoryBank>().is_err(), "{bad} must not parse");
        }
    }
}
