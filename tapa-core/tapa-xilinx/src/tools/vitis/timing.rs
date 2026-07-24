//! Narrow parser for the kernel clock rows in a Vivado timing summary.

use crate::error::{Result, XilinxError};

/// Tolerance for matching a reported clock frequency against the requested
/// kernel frequency. Vivado reports platform clocks to three decimals, and the
/// conservative whole-MHz `--kernel_frequency` rounds, so allow ±1 MHz.
const FREQUENCY_MATCH_TOLERANCE_MHZ: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KernelTiming {
    pub reported_target_period_ns: f64,
    pub reported_target_mhz: f64,
    pub wns_ns: f64,
    pub achieved_period_ns: f64,
    pub fmax_mhz: f64,
}

/// Parse the kernel clock rows from the `Clock Summary` and `Intra Clock Table`.
///
/// The report-wide timing summary can be dominated by an unrelated platform
/// clock, so the kernel result is derived only from these two clock-specific
/// rows.
///
/// `target_mhz` is the whole-MHz frequency passed to `v++ --kernel_frequency`.
/// The kernel clock is identified by its reported `Frequency(MHz)` matching
/// `target_mhz`, which is robust to platform-specific clock names: Vitis names
/// it `ap_clk` on some platforms and `clk_kernel_00_unbuffered_net` (or
/// similar) on Alveo shell platforms. When the target is unknown (`None`),
/// the parser falls back to the historical `ap_clk` name.
pub fn parse_kernel_timing_summary(report: &str, target_mhz: Option<u32>) -> Result<KernelTiming> {
    let kernel_clock = identify_kernel_clock(report, target_mhz)?;

    let clock_row = unique_clock_row(report, "Clock Summary", &kernel_clock)?;
    let intra_row = unique_clock_row(report, "Intra Clock Table", &kernel_clock)?;

    let reported_target_period_ns =
        positive_finite("kernel Period(ns)", parse_field(&clock_row, "Period(ns)")?)?;
    let reported_target_mhz = positive_finite(
        "kernel Frequency(MHz)",
        parse_field(&clock_row, "Frequency(MHz)")?,
    )?;
    let wns_ns = finite("kernel WNS(ns)", parse_field(&intra_row, "WNS(ns)")?)?;
    let achieved_period_ns = reported_target_period_ns - wns_ns;
    let achieved_period_ns = positive_finite("kernel achieved period", achieved_period_ns)?;
    let fmax_mhz = positive_finite("kernel Fmax", 1000.0 / achieved_period_ns)?;

    Ok(KernelTiming {
        reported_target_period_ns,
        reported_target_mhz,
        wns_ns,
        achieved_period_ns,
        fmax_mhz,
    })
}

/// Decide which clock row in the `Clock Summary` is the kernel clock.
///
/// Preference order:
/// 1. A clock whose reported `Frequency(MHz)` matches `target_mhz` (within
///    tolerance). This is the robust, platform-agnostic signal.
/// 2. Otherwise a clock literally named `ap_clk` (the historical TAPA / HLS
///    convention).
fn identify_kernel_clock(report: &str, target_mhz: Option<u32>) -> Result<String> {
    let clocks = clock_rows(report, "Clock Summary")?;
    let by_frequency = target_mhz.map(f64::from).and_then(|target| {
        clocks
            .iter()
            .find_map(|row| {
                let freq = parse_field(row, "Frequency(MHz)").ok()?;
                ((freq - target).abs() <= FREQUENCY_MATCH_TOLERANCE_MHZ).then(|| row.clock_name())
            })
            .map(str::to_string)
    });
    if let Some(clock) = by_frequency {
        return Ok(clock);
    }

    clocks
        .iter()
        .find_map(|row| (row.clock_name() == "ap_clk").then(|| row.clock_name().to_string()))
        .ok_or_else(|| {
            let detail = match target_mhz {
                Some(target) => {
                    format!(
                        "no kernel clock matching target frequency {target} MHz or named `ap_clk` \
                         in `Clock Summary`"
                    )
                }
                None => "missing `ap_clk` row in `Clock Summary`".to_string(),
            };
            XilinxError::TimingSummaryParse(detail)
        })
}

/// Collect every clock data row in a named section's table.
fn clock_rows(report: &str, title: &str) -> Result<Vec<TableRow>> {
    let lines: Vec<&str> = report.lines().collect();
    let marker = format!("| {title}");
    let section_starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (line.trim() == marker).then_some(index))
        .collect();
    let section_start = match section_starts.as_slice() {
        [index] => *index,
        [] => return parse_error(format!("missing `{title}` section")),
        _ => return parse_error(format!("ambiguous `{title}` sections")),
    };

    let header_index = lines
        .iter()
        .enumerate()
        .skip(section_start + 1)
        .take(16)
        .find_map(|(index, line)| {
            let columns = split_columns(line);
            (columns.first() == Some(&"Clock") && columns.len() >= 2).then_some(index)
        })
        .ok_or_else(|| {
            XilinxError::TimingSummaryParse(format!("missing table header in `{title}`"))
        })?;
    let headers: Vec<String> = split_columns(lines[header_index])
        .into_iter()
        .map(str::to_string)
        .collect();

    let mut rows = Vec::new();
    let mut saw_separator = false;
    for line in lines.iter().skip(header_index + 1) {
        let trimmed = line.trim();
        if !saw_separator {
            if trimmed.starts_with('-') {
                saw_separator = true;
            }
            continue;
        }
        if trimmed.is_empty() {
            break;
        }
        let values = split_columns(line);
        if !values.is_empty() {
            rows.push(TableRow {
                headers: headers.clone(),
                values: values.into_iter().map(str::to_string).collect(),
            });
        }
    }
    Ok(rows)
}

fn unique_clock_row(report: &str, title: &str, clock: &str) -> Result<TableRow> {
    let mut matching = clock_rows(report, title)?
        .into_iter()
        .filter(|row| row.clock_name() == clock)
        .collect::<Vec<_>>();
    let values = match matching.as_mut_slice() {
        [row] => row.values.clone(),
        [] => return parse_error(format!("missing `{clock}` row in `{title}`")),
        _ => return parse_error(format!("ambiguous `{clock}` rows in `{title}`")),
    };
    let headers = matching[0].headers.clone();
    if headers.len() != values.len() {
        return parse_error(format!(
            "malformed `{clock}` row in `{title}`: expected {} columns, found {}",
            headers.len(),
            values.len()
        ));
    }
    Ok(TableRow { headers, values })
}

#[derive(Debug)]
struct TableRow {
    headers: Vec<String>,
    values: Vec<String>,
}

impl TableRow {
    fn clock_name(&self) -> &str {
        self.values
            .first()
            .map(String::as_str)
            .unwrap_or_default()
            .trim()
    }
}

fn split_columns(line: &str) -> Vec<&str> {
    line.split("  ")
        .map(str::trim)
        .filter(|column| !column.is_empty())
        .collect()
}

fn parse_field(row: &TableRow, name: &str) -> Result<f64> {
    let index = row
        .headers
        .iter()
        .position(|header| header == name)
        .ok_or_else(|| XilinxError::TimingSummaryParse(format!("missing `{name}` column")))?;
    let value = row.values.get(index).ok_or_else(|| {
        XilinxError::TimingSummaryParse(format!("missing value for `{name}` column"))
    })?;
    value
        .parse::<f64>()
        .map_err(|_| XilinxError::TimingSummaryParse(format!("invalid `{name}` value `{value}`")))
}

fn finite(label: &str, value: f64) -> Result<f64> {
    if value.is_finite() {
        Ok(value)
    } else {
        parse_error(format!("{label} must be finite, got `{value}`"))
    }
}

fn positive_finite(label: &str, value: f64) -> Result<f64> {
    if value.is_finite() && value > 0.0 {
        Ok(value)
    } else {
        parse_error(format!(
            "{label} must be finite and positive, got `{value}`"
        ))
    }
}

fn parse_error<T>(detail: String) -> Result<T> {
    Err(XilinxError::TimingSummaryParse(detail))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(period: &str, frequency: &str, kernel_wns: &str) -> String {
        format!(
            "
| Design Timing Summary
| ---------------------

WNS(ns)      TNS(ns)
-------      -------
-9.000       -10.000

| Clock Summary
| -------------

Clock   Waveform(ns)         Period(ns)      Frequency(MHz)
-----   ------------         ----------      --------------
ap_clk  {{0.000 1.250}}        {period}           {frequency}
other   {{0.000 0.500}}        1.000           1000.000

| Intra Clock Table
| -----------------

Clock             WNS(ns)      TNS(ns)
-----             -------      -------
ap_clk             {kernel_wns}       -604.935
other               0.500         0.000

| Inter Clock Table
| -----------------
"
        )
    }

    #[test]
    fn parses_kernel_rows_instead_of_global_wns() {
        let timing = parse_kernel_timing_summary(&report("2.500", "400.000", "-0.173"), Some(400))
            .expect("valid timing summary");
        assert!((timing.reported_target_period_ns - 2.5).abs() < 1e-12);
        assert!((timing.reported_target_mhz - 400.0).abs() < 1e-12);
        assert!((timing.wns_ns - (-0.173)).abs() < 1e-12);
        assert!((timing.achieved_period_ns - 2.673).abs() < 1e-12);
        assert!((timing.fmax_mhz - (1000.0 / 2.673)).abs() < 1e-12);
    }

    #[test]
    fn positive_slack_reduces_achieved_period() {
        let timing = parse_kernel_timing_summary(&report("2.500", "400.000", "0.200"), Some(400))
            .expect("valid timing summary");
        assert!((timing.achieved_period_ns - 2.3).abs() < 1e-12);
        assert!((timing.fmax_mhz - (1000.0 / 2.3)).abs() < 1e-12);
    }

    #[test]
    fn rejects_missing_or_ambiguous_kernel_rows() {
        // No clock at the target frequency AND no `ap_clk` fallback: the parser
        // cannot identify the kernel clock.
        let no_match = report("2.500", "400.000", "-0.173").replace("ap_clk", "kernel_clk");
        parse_kernel_timing_summary(&no_match, Some(500)).expect_err("unmatched target must fail");

        // `target_mhz = None` falls back to the `ap_clk` name; removing it fails.
        let no_ap_clk = report("2.500", "400.000", "-0.173").replace("ap_clk", "kernel_clk");
        parse_kernel_timing_summary(&no_ap_clk, None).expect_err("missing ap_clk must fail");

        let duplicate_clock = report("2.500", "400.000", "-0.173").replace(
            "other   {0.000 0.500}        1.000           1000.000",
            "ap_clk  {0.000 0.500}        1.000           1000.000",
        );
        parse_kernel_timing_summary(&duplicate_clock, Some(400))
            .expect_err("duplicate Clock Summary row must fail");

        let duplicate_intra = report("2.500", "400.000", "-0.173").replace(
            "other               0.500         0.000",
            "ap_clk              0.500         0.000",
        );
        parse_kernel_timing_summary(&duplicate_intra, Some(400))
            .expect_err("duplicate Intra Clock row must fail");
    }

    #[test]
    fn identifies_kernel_clock_by_frequency_on_non_ap_clk_platforms() {
        // Alveo shell platforms name the kernel clock `clk_kernel_00_unbuffered_net`
        // and expose many unrelated platform clocks. The parser must pick the one
        // whose reported frequency matches the requested target.
        let alveo = "
| Design Timing Summary
| ---------------------

WNS(ns)      TNS(ns)
-------      -------
-1.548       -127819.555

| Clock Summary
| -------------

Clock                          Waveform(ns)         Period(ns)      Frequency(MHz)
-----                          ------------         ----------      --------------
io_clk_freerun_00_clk_p        {0.000 5.000}        10.000          100.000
clk_kernel_00_unbuffered_net   {0.000 1.500}        3.000           333.333
clk_kernel_01_unbuffered_net   {0.000 1.000}        2.000           500.000
hbm_aclk                       {0.000 1.111}        2.222           450.000

| Intra Clock Table
| -----------------

Clock                          WNS(ns)      TNS(ns)
-----                          -------      -------
clk_kernel_00_unbuffered_net   -1.548       -127819.555
clk_kernel_01_unbuffered_net   0.445        0.000

| Inter Clock Table
| -----------------
";
        let timing = parse_kernel_timing_summary(alveo, Some(333)).expect("alveo parse");
        assert!((timing.reported_target_period_ns - 3.0).abs() < 1e-9);
        assert!((timing.reported_target_mhz - 333.333).abs() < 1e-3);
        assert!((timing.wns_ns - (-1.548)).abs() < 1e-9);
        assert!((timing.achieved_period_ns - 4.548).abs() < 1e-9);
        assert!((timing.fmax_mhz - (1000.0 / 4.548)).abs() < 1e-9);
    }

    #[test]
    fn rejects_invalid_source_values_and_denominator() {
        // `None` selects `ap_clk` by name so the value validation below is the
        // thing under test, not clock identification.
        for invalid in ["0", "-1", "NaN", "inf"] {
            assert!(
                parse_kernel_timing_summary(&report(invalid, "400.000", "-0.173"), None).is_err(),
                "period {invalid} must fail",
            );
            assert!(
                parse_kernel_timing_summary(&report("2.500", invalid, "-0.173"), None).is_err(),
                "frequency {invalid} must fail",
            );
        }
        for invalid in ["NaN", "inf", "-inf"] {
            assert!(
                parse_kernel_timing_summary(&report("2.500", "400.000", invalid), None).is_err(),
                "WNS {invalid} must fail",
            );
        }
        for wns in ["2.500", "3.000"] {
            assert!(
                parse_kernel_timing_summary(&report("2.500", "400.000", wns), Some(400)).is_err(),
                "nonpositive achieved period from WNS {wns} must fail",
            );
        }
    }

    #[test]
    fn rejects_duplicate_sections() {
        let one = report("2.500", "400.000", "-0.173");
        let duplicate = format!("{one}\n{one}");
        parse_kernel_timing_summary(&duplicate, Some(400)).expect_err("duplicate sections fail");
    }
}
