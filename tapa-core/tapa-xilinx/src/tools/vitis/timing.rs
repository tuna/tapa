//! Narrow parser for the kernel clock rows in a Vivado timing summary.

use crate::error::{Result, XilinxError};

const KERNEL_CLOCK: &str = "ap_clk";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KernelTiming {
    pub reported_target_period_ns: f64,
    pub reported_target_mhz: f64,
    pub wns_ns: f64,
    pub achieved_period_ns: f64,
    pub fmax_mhz: f64,
}

/// Parse the `ap_clk` rows from the `Clock Summary` and `Intra Clock Table`.
///
/// The report-wide timing summary can be dominated by an unrelated platform
/// clock, so the kernel result is derived only from these two clock-specific
/// rows.
pub fn parse_kernel_timing_summary(report: &str) -> Result<KernelTiming> {
    let clock_row = unique_clock_row(report, "Clock Summary", KERNEL_CLOCK)?;
    let intra_row = unique_clock_row(report, "Intra Clock Table", KERNEL_CLOCK)?;

    let reported_target_period_ns =
        positive_finite("ap_clk Period(ns)", parse_field(&clock_row, "Period(ns)")?)?;
    let reported_target_mhz = positive_finite(
        "ap_clk Frequency(MHz)",
        parse_field(&clock_row, "Frequency(MHz)")?,
    )?;
    let wns_ns = finite("ap_clk WNS(ns)", parse_field(&intra_row, "WNS(ns)")?)?;
    let achieved_period_ns = reported_target_period_ns - wns_ns;
    let achieved_period_ns = positive_finite("ap_clk achieved period", achieved_period_ns)?;
    let fmax_mhz = positive_finite("ap_clk Fmax", 1000.0 / achieved_period_ns)?;

    Ok(KernelTiming {
        reported_target_period_ns,
        reported_target_mhz,
        wns_ns,
        achieved_period_ns,
        fmax_mhz,
    })
}

#[derive(Debug)]
struct TableRow {
    headers: Vec<String>,
    values: Vec<String>,
}

fn unique_clock_row(report: &str, title: &str, clock: &str) -> Result<TableRow> {
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
        if values.first() == Some(&clock) {
            rows.push(values.into_iter().map(str::to_string).collect::<Vec<_>>());
        }
    }

    let values = match rows.as_slice() {
        [values] => values.clone(),
        [] => return parse_error(format!("missing `{clock}` row in `{title}`")),
        _ => return parse_error(format!("ambiguous `{clock}` rows in `{title}`")),
    };
    if headers.len() != values.len() {
        return parse_error(format!(
            "malformed `{clock}` row in `{title}`: expected {} columns, found {}",
            headers.len(),
            values.len()
        ));
    }
    Ok(TableRow { headers, values })
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
        let timing = parse_kernel_timing_summary(&report("2.500", "400.000", "-0.173"))
            .expect("valid timing summary");
        assert!((timing.reported_target_period_ns - 2.5).abs() < 1e-12);
        assert!((timing.reported_target_mhz - 400.0).abs() < 1e-12);
        assert!((timing.wns_ns - (-0.173)).abs() < 1e-12);
        assert!((timing.achieved_period_ns - 2.673).abs() < 1e-12);
        assert!((timing.fmax_mhz - (1000.0 / 2.673)).abs() < 1e-12);
    }

    #[test]
    fn positive_slack_reduces_achieved_period() {
        let timing = parse_kernel_timing_summary(&report("2.500", "400.000", "0.200"))
            .expect("valid timing summary");
        assert!((timing.achieved_period_ns - 2.3).abs() < 1e-12);
        assert!((timing.fmax_mhz - (1000.0 / 2.3)).abs() < 1e-12);
    }

    #[test]
    fn rejects_missing_or_ambiguous_kernel_rows() {
        let missing = report("2.500", "400.000", "-0.173").replace("ap_clk", "kernel_clk");
        parse_kernel_timing_summary(&missing).expect_err("missing ap_clk must fail");

        let duplicate_clock = report("2.500", "400.000", "-0.173").replace(
            "other   {0.000 0.500}        1.000           1000.000",
            "ap_clk  {0.000 0.500}        1.000           1000.000",
        );
        parse_kernel_timing_summary(&duplicate_clock)
            .expect_err("duplicate Clock Summary row must fail");

        let duplicate_intra = report("2.500", "400.000", "-0.173").replace(
            "other               0.500         0.000",
            "ap_clk              0.500         0.000",
        );
        parse_kernel_timing_summary(&duplicate_intra)
            .expect_err("duplicate Intra Clock row must fail");
    }

    #[test]
    fn rejects_invalid_source_values_and_denominator() {
        for invalid in ["0", "-1", "NaN", "inf"] {
            assert!(
                parse_kernel_timing_summary(&report(invalid, "400.000", "-0.173")).is_err(),
                "period {invalid} must fail",
            );
            assert!(
                parse_kernel_timing_summary(&report("2.500", invalid, "-0.173")).is_err(),
                "frequency {invalid} must fail",
            );
        }
        for invalid in ["NaN", "inf", "-inf"] {
            assert!(
                parse_kernel_timing_summary(&report("2.500", "400.000", invalid)).is_err(),
                "WNS {invalid} must fail",
            );
        }
        for wns in ["2.500", "3.000"] {
            assert!(
                parse_kernel_timing_summary(&report("2.500", "400.000", wns)).is_err(),
                "nonpositive achieved period from WNS {wns} must fail",
            );
        }
    }

    #[test]
    fn rejects_duplicate_sections() {
        let one = report("2.500", "400.000", "-0.173");
        let duplicate = format!("{one}\n{one}");
        parse_kernel_timing_summary(&duplicate).expect_err("duplicate sections must fail");
    }
}
