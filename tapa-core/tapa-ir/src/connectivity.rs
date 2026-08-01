//! Plain-data model for external memory connectivity.

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

/// A kind of external memory bank supported by the connectivity schema.
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

/// A repeated endpoint that would violate [`MemoryBindings`]' uniqueness invariant.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "memory endpoint `{endpoint}` {}",
    duplicate_detail(*previous_bank, *new_bank)
)]
pub struct DuplicateMemoryEndpoint {
    /// The endpoint bound more than once.
    pub endpoint: MemoryEndpoint,
    /// The bank the first binding assigned.
    pub previous_bank: MemoryBank,
    /// The bank the rejected binding tried to assign.
    pub new_bank: MemoryBank,
}

fn duplicate_detail(previous_bank: MemoryBank, new_bank: MemoryBank) -> String {
    if previous_bank == new_bank {
        "is repeated".to_string()
    } else {
        format!("maps to both {previous_bank} and {new_bank}")
    }
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
        Self::try_from_bindings(wire.bindings).map_err(serde::de::Error::custom)
    }
}

impl MemoryBindings {
    /// Build bindings while enforcing endpoint uniqueness.
    pub fn try_from_bindings(
        bindings: Vec<MemoryBinding>,
    ) -> Result<Self, DuplicateMemoryEndpoint> {
        let mut endpoints = BTreeMap::<&MemoryEndpoint, MemoryBank>::new();
        for binding in &bindings {
            if let Some(previous_bank) = endpoints.insert(&binding.endpoint, binding.bank) {
                return Err(DuplicateMemoryEndpoint {
                    endpoint: binding.endpoint.clone(),
                    previous_bank,
                    new_bank: binding.bank,
                });
            }
        }
        Ok(Self { bindings })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip_has_stable_snake_case_form() {
        let bindings = MemoryBindings::try_from_bindings(vec![
            MemoryBinding {
                endpoint: MemoryEndpoint {
                    kernel: "kernel_1".to_string(),
                    port: "input".to_string(),
                },
                bank: MemoryBank {
                    kind: MemoryKind::Hbm,
                    index: 3,
                },
            },
            MemoryBinding {
                endpoint: MemoryEndpoint {
                    kernel: "kernel_1".to_string(),
                    port: "output".to_string(),
                },
                bank: MemoryBank {
                    kind: MemoryKind::Ddr,
                    index: 1,
                },
            },
        ])
        .expect("unique bindings");
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
    fn constructor_rejects_duplicate_endpoints() {
        let binding = MemoryBinding {
            endpoint: MemoryEndpoint {
                kernel: "k".to_string(),
                port: "p".to_string(),
            },
            bank: MemoryBank {
                kind: MemoryKind::Hbm,
                index: 0,
            },
        };
        let error = MemoryBindings::try_from_bindings(vec![binding.clone(), binding.clone()])
            .expect_err("repeated endpoint must fail");
        assert_eq!(error.to_string(), "memory endpoint `k.p` is repeated");

        let conflicting = MemoryBinding {
            bank: MemoryBank {
                kind: MemoryKind::Ddr,
                index: 1,
            },
            ..binding.clone()
        };
        let error = MemoryBindings::try_from_bindings(vec![binding, conflicting])
            .expect_err("conflicting endpoint must fail");
        assert_eq!(
            error.to_string(),
            "memory endpoint `k.p` maps to both HBM[0] and DDR[1]"
        );
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
