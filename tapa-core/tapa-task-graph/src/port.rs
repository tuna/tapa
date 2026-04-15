//! Port and category types.

use serde::{Deserialize, Serialize};

/// Argument / port category.
///
/// Covers all 10 wire strings from Python's `Instance.Arg._CAT_LOOKUP`.
/// `hmap` is an alias that deserializes to `Mmap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArgCategory {
    Istream,
    Ostream,
    Istreams,
    Ostreams,
    Scalar,
    Mmap,
    Immap,
    Ommap,
    AsyncMmap,
    /// `"hmap"` in JSON — deserializes to this variant, serializes as `"hmap"`.
    /// Semantically equivalent to `Mmap`.
    Hmap,
}

impl ArgCategory {
    /// Canonical category (collapses `Hmap` → `Mmap`).
    #[must_use]
    pub fn canonical(self) -> Self {
        match self {
            Self::Hmap => Self::Mmap,
            Self::Istream
            | Self::Ostream
            | Self::Istreams
            | Self::Ostreams
            | Self::Scalar
            | Self::Mmap
            | Self::Immap
            | Self::Ommap
            | Self::AsyncMmap => self,
        }
    }
}

/// An external port of a task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Port {
    /// Port category.
    pub cat: ArgCategory,
    /// Port name.
    pub name: String,
    /// C++ type (e.g. `"float"`, `"const float*"`, `"uint64_t"`).
    #[serde(rename = "type")]
    pub ctype: String,
    /// Bit width.
    pub width: u32,
    /// Channel count for hierarchical memory ports.
    #[serde(default)]
    pub chan_count: Option<u32>,
    /// Channel size for hierarchical memory ports.
    #[serde(default)]
    pub chan_size: Option<u32>,
}
