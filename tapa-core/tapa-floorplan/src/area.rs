//! FPGA resource area (LUT, FF, `BRAM_18K`, DSP, URAM).

use serde::{Deserialize, Serialize};

use crate::FloorplanError;

/// Five-component FPGA resource area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Area {
    pub lut: u64,
    pub ff: u64,
    pub bram_18k: u64,
    pub dsp: u64,
    pub uram: u64,
}

impl Area {
    /// Create a new area, validating all fields are non-negative (always true for u64).
    #[must_use]
    pub fn new(lut: u64, ff: u64, bram_18k: u64, dsp: u64, uram: u64) -> Self {
        Self {
            lut,
            ff,
            bram_18k,
            dsp,
            uram,
        }
    }

    /// Create an area from a dictionary with uppercase keys (LUT, FF, `BRAM_18K`, DSP, URAM).
    ///
    /// # Errors
    ///
    /// Returns an error if any expected key is missing.
    pub fn from_resource_map(map: &serde_json::Map<String, serde_json::Value>) -> Result<Self, FloorplanError> {
        let get = |key: &str| -> Result<u64, FloorplanError> {
            map.get(key)
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| FloorplanError::MissingField(key.to_owned()))
        };
        Ok(Self {
            lut: get("LUT")?,
            ff: get("FF")?,
            bram_18k: get("BRAM_18K")?,
            dsp: get("DSP")?,
            uram: get("URAM")?,
        })
    }

    /// Convert to a dictionary with uppercase keys.
    #[must_use]
    pub fn to_resource_map(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut m = serde_json::Map::new();
        m.insert("LUT".into(), self.lut.into());
        m.insert("FF".into(), self.ff.into());
        m.insert("BRAM_18K".into(), self.bram_18k.into());
        m.insert("DSP".into(), self.dsp.into());
        m.insert("URAM".into(), self.uram.into());
        m
    }

    /// Returns `true` if all resource counts are zero.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lut == 0 && self.ff == 0 && self.bram_18k == 0 && self.dsp == 0 && self.uram == 0
    }

    /// Returns `true` if every component of `self` is `<=` the corresponding component of `other`.
    #[must_use]
    pub fn is_smaller_than(&self, other: &Self) -> bool {
        self.lut <= other.lut
            && self.ff <= other.ff
            && self.bram_18k <= other.bram_18k
            && self.dsp <= other.dsp
            && self.uram <= other.uram
    }
}

impl std::ops::Add for Area {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self {
            lut: self.lut + rhs.lut,
            ff: self.ff + rhs.ff,
            bram_18k: self.bram_18k + rhs.bram_18k,
            dsp: self.dsp + rhs.dsp,
            uram: self.uram + rhs.uram,
        }
    }
}

impl Default for Area {
    fn default() -> Self {
        Self::new(0, 0, 0, 0, 0)
    }
}

/// Sum a slice of areas component-wise.
#[must_use]
pub fn sum_area(areas: &[Area]) -> Area {
    areas.iter().copied().fold(Area::default(), |acc, a| acc + a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_new() {
        let a = Area::new(100, 200, 10, 5, 2);
        assert_eq!(a.lut, 100);
        assert_eq!(a.ff, 200);
        assert_eq!(a.bram_18k, 10);
        assert_eq!(a.dsp, 5);
        assert_eq!(a.uram, 2);
    }

    #[test]
    fn area_is_empty() {
        assert!(Area::default().is_empty());
        assert!(!Area::new(1, 0, 0, 0, 0).is_empty());
    }

    #[test]
    fn area_is_smaller_than() {
        let small = Area::new(10, 20, 5, 2, 1);
        let large = Area::new(100, 200, 50, 20, 10);
        assert!(small.is_smaller_than(&large));
        assert!(!large.is_smaller_than(&small));
        assert!(small.is_smaller_than(&small)); // equal is smaller
    }

    #[test]
    fn area_add() {
        let a = Area::new(10, 20, 5, 2, 1);
        let b = Area::new(30, 40, 15, 8, 4);
        let c = a + b;
        assert_eq!(c, Area::new(40, 60, 20, 10, 5));
    }

    #[test]
    fn sum_area_works() {
        let areas = vec![
            Area::new(10, 20, 5, 2, 1),
            Area::new(30, 40, 15, 8, 4),
            Area::new(5, 10, 3, 1, 0),
        ];
        let total = sum_area(&areas);
        assert_eq!(total, Area::new(45, 70, 23, 11, 5));
    }

    #[test]
    fn sum_area_empty() {
        assert_eq!(sum_area(&[]), Area::default());
    }

    #[test]
    fn area_resource_map_round_trip() {
        let a = Area::new(100, 200, 10, 5, 2);
        let map = a.to_resource_map();
        let b = Area::from_resource_map(&map).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn area_from_resource_map_missing_key() {
        let map = serde_json::Map::new();
        let result = Area::from_resource_map(&map);
        assert!(
            result.is_err(),
            "expected error for empty map"
        );
    }

    #[test]
    fn area_serde_round_trip() {
        let a = Area::new(100, 200, 10, 5, 2);
        let json = serde_json::to_string(&a).unwrap();
        let b: Area = serde_json::from_str(&json).unwrap();
        assert_eq!(a, b);
    }
}
