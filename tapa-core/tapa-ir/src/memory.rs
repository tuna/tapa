//! Validated geometry of an mmap after parent and child metadata are merged.

/// Memory geometry independent of a vendor's address-bus width.
///
/// A plain mmap has no channel shape. An hmap has a nonzero channel count
/// and a power-of-two byte size per channel, including single-channel hmaps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryGeometry {
    data_width: u32,
    channels: Option<(u32, u32)>,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct MemoryGeometryError(String);

impl MemoryGeometry {
    /// Validate merged metadata without changing the serialized port schema.
    pub fn new(
        data_width: u32,
        chan_count: Option<u32>,
        chan_size: Option<u32>,
    ) -> Result<Self, MemoryGeometryError> {
        let channels = match (chan_count, chan_size) {
            (Some(0), _) => return Err(MemoryGeometryError("hmap channel count is 0".into())),
            (None, None) => None,
            (Some(count), Some(size)) => Some((count, size)),
            _ => {
                return Err(MemoryGeometryError(
                    "hmap must specify both chan_count and chan_size".into(),
                ))
            }
        };
        if data_width == 0 || !data_width.is_multiple_of(8) {
            return Err(MemoryGeometryError(format!(
                "M-AXI data width must be a nonzero multiple of 8 bits, got {data_width}"
            )));
        }
        if let Some((_, size)) = channels {
            let bytes = u64::from(size) * u64::from(data_width / 8);
            if bytes == 0 {
                return Err(MemoryGeometryError(
                    "hmap channel size must be greater than zero".into(),
                ));
            }
            if !bytes.is_power_of_two() {
                return Err(MemoryGeometryError(format!(
                    "hmap channel byte size must be a power of two: \
                     chan_size={size} * data_width={data_width} / 8 = {bytes} bytes"
                )));
            }
        }
        Ok(Self {
            data_width,
            channels,
        })
    }

    #[must_use]
    pub const fn data_width(self) -> u32 {
        self.data_width
    }

    /// Channel count and size in elements; absent only for a plain mmap.
    #[must_use]
    pub const fn channels(self) -> Option<(u32, u32)> {
        self.channels
    }

    #[must_use]
    pub const fn is_hmap(self) -> bool {
        self.channels.is_some()
    }

    /// A plain mmap behaves as one channel without becoming an hmap.
    #[must_use]
    pub fn channel_count(self) -> u32 {
        self.channels.map_or(1, |(count, _)| count)
    }

    /// Address bits within an hmap channel. Plain mmaps use the backend's
    /// address-bus width instead; a one-byte hmap channel needs zero bits.
    #[must_use]
    pub fn channel_addr_width(self) -> Option<u32> {
        self.channels
            .map(|(_, size)| (u64::from(size) * u64::from(self.data_width / 8)).ilog2())
    }
}

#[cfg(test)]
mod tests {
    use super::MemoryGeometry;

    #[test]
    fn rejects_invalid_channel_geometry() {
        for (width, count, size, message) in [
            (0, None, None, "nonzero multiple of 8"),
            (7, None, None, "nonzero multiple of 8"),
            (32, Some(2), None, "both chan_count and chan_size"),
            (32, None, Some(1024), "both chan_count and chan_size"),
            (32, Some(0), Some(1024), "channel count is 0"),
            (32, Some(1), Some(0), "greater than zero"),
            (32, Some(1), Some(3), "power of two"),
        ] {
            let err = MemoryGeometry::new(width, count, size).unwrap_err();
            assert!(err.to_string().contains(message), "{err}");
        }
    }
}
