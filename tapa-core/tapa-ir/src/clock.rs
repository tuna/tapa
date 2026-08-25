//! Clock period as a typed quantity.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A clock period, held as whole picoseconds.
///
/// Every producer and consumer spells this as decimal nanoseconds: the
/// `--clock-period` flag, the device default, the HLS report, the published
/// report. Picoseconds are the finest unit any of them carries, so a whole
/// count of them represents each value exactly, orders without float
/// comparison rules, and leaves no room for an empty string standing in for
/// "not measured yet" — that is `Option::None`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(transparent)]
pub struct ClockPeriod(u64);

/// Why a nanosecond quantity could not become a [`ClockPeriod`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClockPeriodError {
    /// The text was not a number at all.
    #[error("clock period `{0}` is not a number")]
    NotANumber(String),
    /// The number was negative, infinite, or NaN.
    #[error("clock period must be a finite, non-negative number of nanoseconds, got `{0}`")]
    OutOfRange(String),
}

impl ClockPeriod {
    /// A zero-length period: what an unmeasured task contributes.
    pub const ZERO: Self = Self(0);

    /// Build from whole picoseconds.
    #[must_use]
    pub const fn from_picoseconds(picoseconds: u64) -> Self {
        Self(picoseconds)
    }

    /// Whole picoseconds.
    #[must_use]
    pub const fn picoseconds(self) -> u64 {
        self.0
    }

    /// Nanoseconds, for vendor tools that want a float.
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        reason = "any period a tool reports is far inside f64's exact integer range"
    )]
    pub fn nanoseconds(self) -> f64 {
        self.0 as f64 / 1000.0
    }

    /// Convert a nanosecond quantity, rounding to the nearest picosecond.
    pub fn from_nanoseconds(nanoseconds: f64) -> Result<Self, ClockPeriodError> {
        let picoseconds = nanoseconds * 1000.0;
        // The upper bound (2**64, exactly representable) keeps the `as u64`
        // below from silently saturating; it also rejects non-finite input.
        if !(0.0..18_446_744_073_709_551_616.0).contains(&picoseconds) {
            return Err(ClockPeriodError::OutOfRange(nanoseconds.to_string()));
        }
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "checked finite and non-negative just above"
        )]
        Ok(Self(picoseconds.round() as u64))
    }

    /// Parse decimal nanoseconds, the form every vendor tool writes.
    pub fn from_nanoseconds_str(text: &str) -> Result<Self, ClockPeriodError> {
        let nanoseconds = text
            .trim()
            .parse::<f64>()
            .map_err(|_| ClockPeriodError::NotANumber(text.to_owned()))?;
        Self::from_nanoseconds(nanoseconds)
            .map_err(|_| ClockPeriodError::OutOfRange(text.to_owned()))
    }
}

impl fmt::Display for ClockPeriod {
    /// Decimal nanoseconds with trailing zeros trimmed: `3330` ps reads
    /// `3.33`, `4000` ps reads `4`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let whole = self.0 / 1000;
        let fraction = self.0 % 1000;
        if fraction == 0 {
            return write!(f, "{whole}");
        }
        let mut digits = format!("{fraction:03}");
        while digits.ends_with('0') {
            digits.pop();
        }
        write!(f, "{whole}.{digits}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nanosecond_text_becomes_whole_picoseconds() {
        assert_eq!(
            ClockPeriod::from_nanoseconds_str("3.33").expect("parse"),
            ClockPeriod::from_picoseconds(3330)
        );
        assert_eq!(
            ClockPeriod::from_nanoseconds_str(" 2.871 ").expect("parse"),
            ClockPeriod::from_picoseconds(2871)
        );
        assert_eq!(
            ClockPeriod::from_nanoseconds_str("0").expect("parse"),
            ClockPeriod::ZERO
        );
    }

    #[test]
    fn a_period_that_is_not_a_finite_nonnegative_number_is_rejected() {
        assert!(matches!(
            ClockPeriod::from_nanoseconds_str("abc"),
            Err(ClockPeriodError::NotANumber(_))
        ));
        for text in ["-1", "NaN", "inf"] {
            assert!(
                matches!(
                    ClockPeriod::from_nanoseconds_str(text),
                    Err(ClockPeriodError::OutOfRange(_))
                ),
                "{text} must be rejected"
            );
        }
    }

    /// The published report prints nanoseconds, so the round trip through
    /// picoseconds has to come back to the same reading.
    #[test]
    fn display_reads_back_as_nanoseconds() {
        for (picoseconds, text) in [
            (3330, "3.33"),
            (4000, "4"),
            (2871, "2.871"),
            (0, "0"),
            (1, "0.001"),
        ] {
            let period = ClockPeriod::from_picoseconds(picoseconds);
            assert_eq!(period.to_string(), text);
            assert_eq!(
                ClockPeriod::from_nanoseconds_str(text).expect("reparse"),
                period
            );
        }
    }

    /// Ordering is what the report's critical path is built on.
    #[test]
    fn periods_order_by_length() {
        assert!(ClockPeriod::from_picoseconds(3330) > ClockPeriod::from_picoseconds(2871));
        assert_eq!(
            [ClockPeriod::from_picoseconds(1), ClockPeriod::ZERO]
                .into_iter()
                .max(),
            Some(ClockPeriod::from_picoseconds(1))
        );
    }

    #[test]
    fn serde_carries_bare_picoseconds() {
        let period = ClockPeriod::from_picoseconds(2871);
        let json = serde_json::to_string(&period).expect("serialize");
        assert_eq!(json, "2871");
        assert_eq!(
            serde_json::from_str::<ClockPeriod>(&json).expect("parse"),
            period
        );
    }
}
