//! HLS report parsers: `csynth.xml` and utilization `.rpt`.
//!
//! `parse_csynth_xml` pulls the well-known top-level scalars that TAPA
//! consumes (top module name, target part, target and estimated clock
//! periods) out of the HLS report XML. `parse_utilization_rpt` ports
//! the hierarchical ASCII-table walk from
//! the implementation.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::{Result, XilinxError};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CsynthReport {
    pub top: String,
    pub part: String,
    pub target_clock_period_ns: String,
    pub estimated_clock_period_ns: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UtilizationReport {
    pub device: String,
    pub instance: String,
    pub metrics: HashMap<String, String>,
    pub children: Vec<Self>,
}

#[derive(Debug, Deserialize)]
struct CsynthXml {
    #[serde(rename = "UserAssignments")]
    user_assignments: UserAssignments,
    #[serde(rename = "PerformanceEstimates")]
    performance_estimates: PerformanceEstimates,
}

#[derive(Debug, Deserialize)]
struct UserAssignments {
    #[serde(rename = "TopModelName", default)]
    top_model_name: Option<String>,
    #[serde(rename = "TopModuleName", default)]
    top_module_name: Option<String>,
    #[serde(rename = "Part")]
    part: String,
    #[serde(rename = "TargetClockPeriod", default)]
    target_clock_period: Option<String>,
    #[serde(rename = "CTargetClockPeriod", default)]
    c_target_clock_period: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PerformanceEstimates {
    #[serde(rename = "SummaryOfTimingAnalysis")]
    summary_of_timing_analysis: SummaryOfTimingAnalysis,
}

#[derive(Debug, Deserialize)]
struct SummaryOfTimingAnalysis {
    #[serde(rename = "EstimatedClockPeriod")]
    estimated_clock_period: String,
}

pub fn parse_csynth_xml(bytes: &[u8]) -> Result<CsynthReport> {
    let parsed: CsynthXml = quick_xml::de::from_reader(bytes).map_err(|e| {
        XilinxError::HlsReportParse(format!("csynth.xml parse failed: {e}"))
    })?;

    let top = parsed
        .user_assignments
        .top_model_name
        .or(parsed.user_assignments.top_module_name)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            XilinxError::HlsReportParse("csynth.xml: TopModuleName not found".into())
        })?;

    let target_cp = parsed
        .user_assignments
        .target_clock_period
        .or(parsed.user_assignments.c_target_clock_period)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            XilinxError::HlsReportParse("csynth.xml: TargetClockPeriod not found".into())
        })?;

    Ok(CsynthReport {
        top: top.trim().to_string(),
        part: parsed.user_assignments.part.trim().to_string(),
        target_clock_period_ns: target_cp.trim().to_string(),
        estimated_clock_period_ns: parsed
            .performance_estimates
            .summary_of_timing_analysis
            .estimated_clock_period
            .trim()
            .to_string(),
    })
}

/// Parse a Vivado hierarchical utilization `.rpt`. Ports
/// the implementation.
pub fn parse_utilization_rpt(text: &str) -> Result<UtilizationReport> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Prolog,
        Header,
        Body,
    }

    let mut state = State::Prolog;
    let mut device = String::new();
    let mut schema: Vec<String> = Vec::new();
    let mut root: Option<UtilizationReport> = None;
    let mut stack: Vec<(usize, Vec<usize>)> = Vec::new(); // (depth, index path into root)

    for raw in text.lines() {
        let line = raw.trim();
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.len() == 4 && words[0..3] == ["|", "Device", ":"] {
            device = words[3].to_string();
            continue;
        }
        if is_ascii_table_rule(line) {
            state = match state {
                State::Prolog => State::Header,
                State::Header => State::Body,
                State::Body => break,
            };
            continue;
        }

        match state {
            State::Header => {
                let (_, cols) = split_row(line);
                schema = cols.iter().map(|s| s.trim().to_string()).collect();
            }
            State::Body => {
                let (inst_raw, cols) = split_row(line);
                let depth = (inst_raw.len() - inst_raw.trim_start_matches(' ').len()) / 2;
                let instance = inst_raw.trim().to_string();
                if schema.len() != cols.len() {
                    return Err(XilinxError::HlsReportParse(
                        "utilization.rpt: column count mismatch".into(),
                    ));
                }
                let metrics: HashMap<String, String> = schema
                    .iter()
                    .cloned()
                    .zip(cols.into_iter().map(|s| s.trim().to_string()))
                    .collect();

                let new = UtilizationReport {
                    device: device.clone(),
                    instance,
                    metrics,
                    children: Vec::new(),
                };

                while stack.last().is_some_and(|(d, _)| *d >= depth) {
                    stack.pop();
                }
                if let Some((_, path)) = stack.last().cloned() {
                    let mut node = root.as_mut().unwrap();
                    for i in &path {
                        node = &mut node.children[*i];
                    }
                    node.children.push(new);
                    let idx = node.children.len() - 1;
                    let mut new_path = path;
                    new_path.push(idx);
                    stack.push((depth, new_path));
                } else {
                    root = Some(new);
                    stack.push((depth, Vec::new()));
                }
            }
            State::Prolog => {}
        }
    }

    root.ok_or_else(|| XilinxError::HlsReportParse("utilization.rpt: no rows parsed".into()))
}

fn split_row(line: &str) -> (&str, Vec<&str>) {
    let trimmed = line.trim().trim_matches('|');
    let mut parts = trimmed.split('|');
    let instance = parts.next().unwrap_or("");
    let cols: Vec<&str> = parts.collect();
    (instance, cols)
}

fn is_ascii_table_rule(line: &str) -> bool {
    !line.is_empty()
        && line.starts_with('+')
        && line
            .chars()
            .all(|c| c == '+' || c == '-' || c.is_ascii_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSYNTH: &str = "<?xml version=\"1.0\"?>
<profile>
  <UserAssignments>
    <TopModelName>vadd</TopModelName>
    <Part>xcu250-figd2104-2L-e</Part>
    <TargetClockPeriod>3.333</TargetClockPeriod>
  </UserAssignments>
  <PerformanceEstimates>
    <SummaryOfTimingAnalysis>
      <EstimatedClockPeriod>2.871</EstimatedClockPeriod>
    </SummaryOfTimingAnalysis>
  </PerformanceEstimates>
</profile>";

    #[test]
    fn parses_csynth_top_fields() {
        let r = parse_csynth_xml(CSYNTH.as_bytes()).unwrap();
        assert_eq!(r.top, "vadd");
        assert_eq!(r.part, "xcu250-figd2104-2L-e");
        assert_eq!(r.target_clock_period_ns, "3.333");
        assert_eq!(r.estimated_clock_period_ns, "2.871");
    }

    #[test]
    fn csynth_missing_field_is_typed_error() {
        let xml =
            "<profile><UserAssignments><TopModelName>k</TopModelName></UserAssignments></profile>";
        let err = parse_csynth_xml(xml.as_bytes()).unwrap_err();
        assert!(matches!(err, XilinxError::HlsReportParse(_)));
    }

    const RPT: &str = "Hierarchical Utilization Report\n\
| Device : xcu250\n\
+----------+-------+------+\n\
| Instance | LUT   | REG  |\n\
+----------+-------+------+\n\
| top      | 100   | 200  |\n\
|   sub    | 30    | 40   |\n\
+----------+-------+------+\n";

    #[test]
    fn parses_hierarchical_utilization() {
        let r = parse_utilization_rpt(RPT).unwrap();
        assert_eq!(r.device, "xcu250");
        assert_eq!(r.instance, "top");
        assert_eq!(r.metrics.get("LUT").unwrap(), "100");
        assert_eq!(r.children.len(), 1);
        assert_eq!(r.children[0].instance, "sub");
        assert_eq!(r.children[0].metrics.get("REG").unwrap(), "40");
    }

    #[test]
    fn parses_vivado_report_with_banner_rules() {
        const RPT: &str = "\
Copyright 1986-2022 Xilinx, Inc. All Rights Reserved.\n\
--------------------------------------------------------------------\n\
| Tool Version : Vivado v.2024.2\n\
| Device       : xcv80-lsva4737-2MHP-e-S\n\
| Design State : Optimized\n\
--------------------------------------------------------------------\n\
\n\
Utilization Design Information\n\
\n\
+----------+--------+------------+-----+------+--------+--------+------+\n\
| Instance | Module | Total LUTs | FFs | URAM | RAMB36 | RAMB18 | DSP Blocks |\n\
+----------+--------+------------+-----+------+--------+--------+------+\n\
| Add      |  (top) |        107 | 140 |    0 |      0 |      0 |          1 |\n\
|   child  |    Add |          5 |   6 |    0 |      0 |      0 |          0 |\n\
+----------+--------+------------+-----+------+--------+--------+------+\n";

        let r = parse_utilization_rpt(RPT).unwrap();
        assert_eq!(r.device, "xcv80-lsva4737-2MHP-e-S");
        assert_eq!(r.instance, "Add");
        assert_eq!(r.metrics.get("Total LUTs").unwrap(), "107");
        assert_eq!(r.metrics.get("FFs").unwrap(), "140");
        assert_eq!(r.metrics.get("DSP Blocks").unwrap(), "1");
        assert_eq!(r.children[0].instance, "child");
    }
}
