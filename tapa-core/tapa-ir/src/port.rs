//! Port and category types.

use regex_lite::Regex;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::LazyLock;
use strum::{EnumString, IntoStaticStr};

static ARRAY_NAME_WITH_SUFFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([a-zA-Z_]\w*)\[(\d+)\]([a-zA-Z_]\w*)?$").unwrap());

/// Collapse `name[idx]` into `name_{idx}`.
///
/// The frontend spells the channels of an array interface (`mmaps`,
/// `streams`) `name[i]`, and the brackets are load-bearing in the graph --
/// they let consumers recover which logical argument a channel belongs to.
/// Every projection to an RTL identifier collapses them, so this lives here,
/// on the schema, rather than in any one consumer: `tapa-codegen`,
/// `tapa pack` and `frt-cosim` must all agree on the name a port binds to.
#[must_use]
pub fn sanitize_array_name(name: &str) -> String {
    ARRAY_NAME_WITH_SUFFIX_RE.captures(name).map_or_else(
        || name.to_owned(),
        |caps| {
            format!(
                "{}_{}{}",
                &caps[1],
                &caps[2],
                caps.get(3).map_or("", |m| m.as_str())
            )
        },
    )
}

/// Argument / port category.
///
/// `"hmap"` is an alias that deserializes to `Mmap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum ArgCategory {
    Istream,
    Ostream,
    Istreams,
    Ostreams,
    Scalar,
    #[strum(serialize = "mmap", serialize = "hmap")]
    Mmap,
    Immap,
    Ommap,
    #[strum(serialize = "async_mmap")]
    AsyncMmap,
}

impl Serialize for ArgCategory {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ArgCategory {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(|_| serde::de::Error::custom(format!("unknown category: {s}")))
    }
}

impl ArgCategory {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        if self == Self::Mmap {
            "mmap"
        } else {
            let s: &'static str = self.into();
            s
        }
    }

    /// Whether this is an input (consumer) stream port.
    #[must_use]
    pub const fn is_input_stream(self) -> bool {
        matches!(self, Self::Istream | Self::Istreams)
    }

    /// Whether this is an output (producer) stream port.
    #[must_use]
    pub const fn is_output_stream(self) -> bool {
        matches!(self, Self::Ostream | Self::Ostreams)
    }

    /// Whether this is any stream port (input or output).
    #[must_use]
    pub const fn is_stream(self) -> bool {
        self.is_input_stream() || self.is_output_stream()
    }

    /// Whether this is a scalar port.
    #[must_use]
    pub const fn is_scalar(self) -> bool {
        matches!(self, Self::Scalar)
    }

    /// Whether this is a memory-mapped port that binds directly to a
    /// top-level M-AXI interface (`mmap`/`async_mmap`), as opposed to
    /// `immap`/`ommap`, which connect two tasks inside the design.
    #[must_use]
    pub const fn is_direct_mmap(self) -> bool {
        matches!(self, Self::Mmap | Self::AsyncMmap)
    }

    /// Whether this is any memory-mapped port.
    #[must_use]
    pub const fn is_mmap_like(self) -> bool {
        matches!(
            self,
            Self::Mmap | Self::Immap | Self::Ommap | Self::AsyncMmap
        )
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chan_count: Option<u32>,
    /// Channel size for hierarchical memory ports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chan_size: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmap_deserializes_to_mmap() {
        let json = r#""hmap""#;
        let cat: ArgCategory = serde_json::from_str(json).expect("parse hmap");
        assert_eq!(cat, ArgCategory::Mmap, "hmap must map to Mmap");
    }

    #[test]
    fn mmap_round_trips_as_mmap() {
        let cat = ArgCategory::Mmap;
        let json = serde_json::to_string(&cat).expect("serialize");
        assert_eq!(json, r#""mmap""#, "Mmap serializes as mmap");
    }

    #[test]
    fn hmap_round_trips_as_mmap() {
        let cat: ArgCategory = serde_json::from_str(r#""hmap""#).expect("parse");
        let json = serde_json::to_string(&cat).expect("serialize");
        assert_eq!(json, r#""mmap""#, "hmap round-trips as mmap");
    }

    #[test]
    fn all_categories_deserialize() {
        let cases = [
            ("istream", ArgCategory::Istream),
            ("ostream", ArgCategory::Ostream),
            ("istreams", ArgCategory::Istreams),
            ("ostreams", ArgCategory::Ostreams),
            ("scalar", ArgCategory::Scalar),
            ("mmap", ArgCategory::Mmap),
            ("immap", ArgCategory::Immap),
            ("ommap", ArgCategory::Ommap),
            ("async_mmap", ArgCategory::AsyncMmap),
            ("hmap", ArgCategory::Mmap),
        ];
        for (s, expected) in cases {
            let json = format!(r#""{s}""#);
            let cat: ArgCategory =
                serde_json::from_str(&json).unwrap_or_else(|e| panic!("failed to parse {s}: {e}"));
            assert_eq!(cat, expected, "category {s}");
        }
    }

    #[test]
    fn is_input_stream_predicate() {
        assert!(ArgCategory::Istream.is_input_stream());
        assert!(ArgCategory::Istreams.is_input_stream());
        assert!(!ArgCategory::Ostream.is_input_stream());
        assert!(!ArgCategory::Scalar.is_input_stream());
        assert!(!ArgCategory::Mmap.is_input_stream());
    }

    #[test]
    fn is_output_stream_predicate() {
        assert!(ArgCategory::Ostream.is_output_stream());
        assert!(ArgCategory::Ostreams.is_output_stream());
        assert!(!ArgCategory::Istream.is_output_stream());
        assert!(!ArgCategory::Scalar.is_output_stream());
    }

    #[test]
    fn is_stream_predicate() {
        assert!(ArgCategory::Istream.is_stream());
        assert!(ArgCategory::Istreams.is_stream());
        assert!(ArgCategory::Ostream.is_stream());
        assert!(ArgCategory::Ostreams.is_stream());
        assert!(!ArgCategory::Scalar.is_stream());
        assert!(!ArgCategory::Mmap.is_stream());
        assert!(!ArgCategory::AsyncMmap.is_stream());
    }

    #[test]
    fn is_scalar_predicate() {
        assert!(ArgCategory::Scalar.is_scalar());
        assert!(!ArgCategory::Istream.is_scalar());
        assert!(!ArgCategory::Mmap.is_scalar());
    }

    #[test]
    fn is_mmap_like_predicate() {
        assert!(ArgCategory::Mmap.is_mmap_like());
        assert!(ArgCategory::Immap.is_mmap_like());
        assert!(ArgCategory::Ommap.is_mmap_like());
        assert!(ArgCategory::AsyncMmap.is_mmap_like());
        assert!(!ArgCategory::Scalar.is_mmap_like());
        assert!(!ArgCategory::Istream.is_mmap_like());
        assert!(!ArgCategory::Ostreams.is_mmap_like());
    }

    #[test]
    fn invalid_category_rejected() {
        let result = serde_json::from_str::<ArgCategory>(r#""nonexistent""#);
        assert!(result.is_err(), "unknown category must be rejected");
    }

    #[test]
    fn sanitize_array_name_collapses_brackets() {
        assert_eq!(sanitize_array_name("foo[3]"), "foo_3");
        assert_eq!(sanitize_array_name("foo[3]_bar"), "foo_3_bar");
        assert_eq!(sanitize_array_name("foo"), "foo");
    }

    /// Only a well-formed `name[idx]` collapses; anything else is passed
    /// through untouched rather than mangled.
    #[test]
    fn sanitize_array_name_passes_through_non_arrays() {
        for name in ["foo[bar]", "foo[]", "[0]", "foo[0", "foo[0][1]"] {
            assert_eq!(
                sanitize_array_name(name),
                name,
                "{name} is not a well-formed array name",
            );
        }
    }
}
