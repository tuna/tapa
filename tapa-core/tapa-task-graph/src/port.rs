//! Port and category types.

use serde::{Deserialize, Serialize};

/// Argument / port category.
///
/// Covers all 10 wire strings from Python's `Instance.Arg._CAT_LOOKUP`.
/// `"hmap"` is an alias that deserializes to `Mmap` (matching Python behavior).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
}

impl ArgCategory {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "istream" => Some(Self::Istream),
            "ostream" => Some(Self::Ostream),
            "istreams" => Some(Self::Istreams),
            "ostreams" => Some(Self::Ostreams),
            "scalar" => Some(Self::Scalar),
            "mmap" | "hmap" => Some(Self::Mmap),
            "immap" => Some(Self::Immap),
            "ommap" => Some(Self::Ommap),
            "async_mmap" => Some(Self::AsyncMmap),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Istream => "istream",
            Self::Ostream => "ostream",
            Self::Istreams => "istreams",
            Self::Ostreams => "ostreams",
            Self::Scalar => "scalar",
            Self::Mmap => "mmap",
            Self::Immap => "immap",
            Self::Ommap => "ommap",
            Self::AsyncMmap => "async_mmap",
        }
    }
}

impl Serialize for ArgCategory {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ArgCategory {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).ok_or_else(|| {
            serde::de::Error::custom(format!("unknown category: {s}"))
        })
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
            let cat: ArgCategory = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("failed to parse {s}: {e}"));
            assert_eq!(cat, expected, "category {s}");
        }
    }

    #[test]
    fn invalid_category_rejected() {
        let result = serde_json::from_str::<ArgCategory>(r#""nonexistent""#);
        assert!(result.is_err(), "unknown category must be rejected");
    }
}
