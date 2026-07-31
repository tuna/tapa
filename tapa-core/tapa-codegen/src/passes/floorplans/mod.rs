//! Floorplan-consumption adapter for the FIFO pass (F1): every
//! `FloorplanResult` read the fifo wiring needs, reading `tapa_ir::floorplan`
//! types only (REFACTOR-PLAN §4 Phase 1 item 2).

use tapa_ir::{FloorplanResult, RoutedChannel};

/// The Body-cell count if `fifo_name` has a floorplanned cross-slot route.
///
/// `reg_regions` is the authoritative per-cell handoff to XDC emission. Using
/// its length here guarantees the generated Body hierarchy and its placement
/// constraints cannot silently disagree.
pub(super) fn stream_crossing_body_level(
    floorplan: Option<&FloorplanResult>,
    fifo_name: &str,
) -> Option<u32> {
    floorplan?
        .routes
        .iter()
        .find_map(|route| match &route.channel {
            RoutedChannel::Stream { fifo } if fifo == fifo_name => {
                Some(u32::try_from(route.reg_regions.len()).unwrap_or(u32::MAX))
            }
            RoutedChannel::Stream { .. }
            | RoutedChannel::Axi { .. }
            | RoutedChannel::Control { .. } => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossing_body_count_comes_from_xdc_region_list() {
        use std::collections::BTreeMap;
        use tapa_ir::{PipelineRoute, PipelineScheme};

        let floorplan = FloorplanResult {
            device: "u280".to_string(),
            grid: (2, 3),
            regions: BTreeMap::new(),
            routes: vec![PipelineRoute {
                channel: RoutedChannel::Stream {
                    fifo: "data_q".to_string(),
                },
                route: vec!["SLOT_X0Y0".to_string(), "SLOT_X0Y1".to_string()],
                scheme: PipelineScheme::Double,
                reg_regions: vec!["SLOT_X0Y0".to_string(), "SLOT_X0Y1".to_string()],
            }],
            slot_usage: BTreeMap::new(),
        };
        assert_eq!(
            stream_crossing_body_level(Some(&floorplan), "data_q"),
            Some(2)
        );
    }
}
