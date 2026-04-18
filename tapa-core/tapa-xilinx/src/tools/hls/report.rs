//! HLS report parsers: `csynth.xml` and utilization `.rpt`.
//!
//! `parse_csynth_xml` pulls the well-known top-level scalars that TAPA
//! consumes (top module name, target part, target and estimated clock
//! periods) out of the HLS report XML. `parse_utilization_rpt` ports
//! the hierarchical ASCII-table walk from
//! `tapa/backend/report/xilinx/rtl/parser.py`.

use std::collections::HashMap;

use quick_xml::events::Event;
use quick_xml::Reader;
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

pub fn parse_csynth_xml(bytes: &[u8]) -> Result<CsynthReport> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);

    let mut path: Vec<String> = Vec::new();
    let mut top: Option<String> = None;
    let mut part: Option<String> = None;
    let mut target_cp: Option<String> = None;
    let mut estimated_cp: Option<String> = None;
    let mut buf = Vec::new();
    let mut current_text = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            // Route malformed csynth.xml to the HLS-scoped variant so
            // the AC-5 negative contract (truncated report surfaces as
            // `HlsReportParse`) holds end-to-end.
            Err(e) => {
                return Err(XilinxError::HlsReportParse(format!(
                    "csynth.xml parse failed: {e}"
                )));
            }
            Ok(Event::Eof) => break,
            Ok(Event::Start(ref e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                path.push(name);
                current_text.clear();
            }
            Ok(Event::Text(t)) => {
                let s = t.unescape().map_err(|e| {
                    XilinxError::HlsReportParse(format!(
                        "csynth.xml text unescape failed: {e}"
                    ))
                })?.into_owned();
                current_text.push_str(&s);
            }
            Ok(Event::End(_)) => {
                let leaf = path.last().map_or("", String::as_str);
                match leaf {
                    "TopModelName" | "TopModuleName" => {
                        if top.is_none() && !current_text.is_empty() {
                            top = Some(current_text.trim().to_string());
                        }
                    }
                    "Part" => {
                        if part.is_none() && !current_text.is_empty() {
                            part = Some(current_text.trim().to_string());
                        }
                    }
                    "TargetClockPeriod" | "CTargetClockPeriod" => {
                        if target_cp.is_none() && !current_text.is_empty() {
                            target_cp = Some(current_text.trim().to_string());
                        }
                    }
                    "EstimatedClockPeriod" => {
                        if estimated_cp.is_none() && !current_text.is_empty() {
                            estimated_cp = Some(current_text.trim().to_string());
                        }
                    }
                    _ => {}
                }
                path.pop();
                current_text.clear();
            }
            Ok(_) => {}
        }
        buf.clear();
    }

    Ok(CsynthReport {
        top: top.ok_or_else(|| {
            XilinxError::HlsReportParse("csynth.xml: TopModuleName not found".into())
        })?,
        part: part.ok_or_else(|| {
            XilinxError::HlsReportParse("csynth.xml: Part not found".into())
        })?,
        target_clock_period_ns: target_cp.ok_or_else(|| {
            XilinxError::HlsReportParse("csynth.xml: TargetClockPeriod not found".into())
        })?,
        estimated_clock_period_ns: estimated_cp.ok_or_else(|| {
            XilinxError::HlsReportParse("csynth.xml: EstimatedClockPeriod not found".into())
        })?,
    })
}

/// Parse a Vivado hierarchical utilization `.rpt`. Ports
/// `tapa/backend/report/xilinx/rtl/parser.py`.
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
        if !line.is_empty() && line.chars().all(|c| c == '+' || c == '-') {
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
        let xml = "<profile><UserAssignments><TopModelName>k</TopModelName></UserAssignments></profile>";
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
}
