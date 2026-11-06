//! Built-in SLASH platform table.
//!
//! A SLASH shell installs no `.xpfm` directory for
//! [`super::device::parse_device_info`] to read — slashkit carries the
//! static shell itself — so `tapa synth --platform` resolves these names
//! from a typed constant, consulted before any platform-directory
//! search. The table is the single home of each shell's part number and
//! default kernel clock period.

/// One built-in SLASH shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlashPlatform {
    /// The `--platform` spelling the CLI accepts.
    pub name: &'static str,
    /// The Xilinx part number HLS and implementation target.
    pub part_num: &'static str,
    /// Default kernel clock period in decimal nanoseconds, the form the
    /// vendor tools and `DeviceInfo::clock_period` speak.
    pub default_clock_period: &'static str,
    /// SLASH's `[connectivity] shell=` value.
    pub shell: &'static str,
    /// Supported dedicated memory interfaces, checked before invoking SLASH.
    pub hbm_ports: u32,
    pub ddr_ports: u32,
}

/// The built-in SLASH shells.
pub const SLASH_PLATFORMS: &[SlashPlatform] = &[
    SlashPlatform {
        name: "slash_v80_service",
        part_num: "xcv80-lsva4737-2MHP-e-S",
        // 3.33 ns (300 MHz): the measured vadd default on xcv80, WNS
        // +0.214 ns out of context in Vivado 2025.2.
        default_clock_period: "3.33",
        shell: "service",
        hbm_ports: 64,
        ddr_ports: 4,
    },
    SlashPlatform {
        name: "slash_v80_compute",
        part_num: "xcv80-lsva4737-2MHP-e-S",
        // Target 300 MHz, matching the service shell default.
        default_clock_period: "3.33",
        shell: "compute",
        hbm_ports: 64,
        ddr_ports: 12,
    },
];

/// Look up a built-in SLASH shell by its `--platform` spelling.
#[must_use]
pub fn find_slash_platform(name: &str) -> Option<&'static SlashPlatform> {
    SLASH_PLATFORMS
        .iter()
        .find(|platform| platform.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v80_shells_share_the_part_and_default_clock() {
        for name in ["slash_v80_service", "slash_v80_compute"] {
            let shell = find_slash_platform(name).expect("known shell");
            assert_eq!(shell.part_num, "xcv80-lsva4737-2MHP-e-S");
            assert_eq!(shell.default_clock_period, "3.33");
        }
    }

    #[test]
    fn unknown_name_finds_nothing() {
        assert!(find_slash_platform("xilinx_u250_gen3x16_xdma_4_1_202210_1").is_none());
        assert!(find_slash_platform("").is_none());
    }

    #[test]
    fn table_names_are_unique_and_clocks_parse() {
        for (index, shell) in SLASH_PLATFORMS.iter().enumerate() {
            assert!(
                SLASH_PLATFORMS
                    .iter()
                    .filter(|s| s.name == shell.name)
                    .count()
                    == 1,
                "shell name `{}` is not unique",
                shell.name,
            );
            tapa_ir::ClockPeriod::from_nanoseconds_str(shell.default_clock_period)
                .unwrap_or_else(|error| panic!("shell `{}` default clock: {error}", shell.name));
            assert!(!shell.part_num.is_empty(), "shell {index} has no part");
        }
    }
}
