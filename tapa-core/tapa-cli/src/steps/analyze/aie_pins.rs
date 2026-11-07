//! `tapa analyze --aie-location` and `--aie-plio`: AI Engine placement and
//! PLIO widths, given on the command line so a source stays device-neutral.
//! They land in the task graph exactly where synthesis and `aiesim` read
//! them.

use tapa_ir::task::AieLocation;
use tapa_ir::{ArgCategory, Graph, SynthTarget, Task, TaskLevel};

use crate::error::{CliError, Result};

/// One `--aie-location TASK:COL,ROW[,INSTANCE]` tile pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AieLocationArg {
    task: String,
    location: AieLocation,
}

/// One `--aie-plio TASK.PORT:WIDTH` PLIO width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiePlioArg {
    task: String,
    port: String,
    width: u32,
}

pub fn parse_aie_location(value: &str) -> std::result::Result<AieLocationArg, String> {
    let usage = || format!("`{value}` is not TASK:COL,ROW or TASK:COL,ROW,INSTANCE");
    let (task, tile) = value.rsplit_once(':').ok_or_else(usage)?;
    let numbers = tile
        .split(',')
        .map(str::parse::<u32>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| usage())?;
    let (col, row, instance) = match numbers[..] {
        [col, row] => (col, row, None),
        [col, row, instance] => (col, row, Some(instance)),
        _ => return Err(usage()),
    };
    if task.is_empty() {
        return Err(usage());
    }
    Ok(AieLocationArg {
        task: task.to_string(),
        location: AieLocation { col, row, instance },
    })
}

pub fn parse_aie_plio(value: &str) -> std::result::Result<AiePlioArg, String> {
    let usage = || format!("`{value}` is not TASK.PORT:WIDTH with a WIDTH of 32, 64 or 128");
    let (endpoint, width) = value.rsplit_once(':').ok_or_else(usage)?;
    let (task, port) = endpoint.rsplit_once('.').ok_or_else(usage)?;
    let width = width.parse::<u32>().map_err(|_| usage())?;
    if task.is_empty() || port.is_empty() || ![32, 64, 128].contains(&width) {
        return Err(usage());
    }
    Ok(AiePlioArg {
        task: task.to_string(),
        port: port.to_string(),
        width,
    })
}

/// Records the pins and widths in `graph`, rejecting any that names no AI
/// Engine task, or a port that is not one of its streams.
pub fn apply(graph: &mut Graph, locations: &[AieLocationArg], plios: &[AiePlioArg]) -> Result<()> {
    for pin in locations {
        let flag = format!("--aie-location {}", pin.task);
        let task = aie_task(graph, &pin.task, &flag)?;
        if task.level != TaskLevel::Lower {
            return Err(CliError::InvalidArg(format!(
                "`{flag}` pins an AI Engine leaf task to a tile, but `{}` is an upper task",
                pin.task
            )));
        }
        if !task.aie_location.contains(&pin.location) {
            task.aie_location.push(pin.location);
        }
    }
    for plio in plios {
        let flag = format!("--aie-plio {}.{}", plio.task, plio.port);
        let task = aie_task(graph, &plio.task, &flag)?;
        let port = task
            .ports
            .iter_mut()
            .find(|port| {
                port.name == plio.port
                    && matches!(
                        port.cat,
                        ArgCategory::Istream
                            | ArgCategory::Ostream
                            | ArgCategory::Iframes
                            | ArgCategory::Oframes
                    )
            })
            .ok_or_else(|| {
                CliError::InvalidArg(format!(
                    "`{flag}` names no single stream port of `{}`",
                    plio.task
                ))
            })?;
        port.aie_plio = Some(plio.width);
    }
    Ok(())
}

fn aie_task<'g>(graph: &'g mut Graph, name: &str, flag: &str) -> Result<&'g mut Task> {
    let task = graph
        .tasks
        .get_mut(name)
        .ok_or_else(|| CliError::InvalidArg(format!("`{flag}` names no task `{name}`")))?;
    if task.synth != SynthTarget::Aie {
        return Err(CliError::InvalidArg(format!(
            "`{flag}` names `{name}`, which is not an AI Engine task"
        )));
    }
    Ok(task)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tapa_ir::Target;

    use super::*;
    use crate::testutil::{port, task};

    fn graph() -> Graph {
        let aie = |name: &str, level: TaskLevel| Task {
            level,
            synth: SynthTarget::Aie,
            ports: vec![
                port("a", ArgCategory::Istream, "float", 32),
                port("n", ArgCategory::Scalar, "int", 32),
            ],
            ..task(name)
        };
        Graph {
            schema_version: tapa_ir::graph::SCHEMA_VERSION,
            top: "Top".to_string(),
            target: Target::XilinxVitis,
            cflags: Vec::new(),
            tasks: BTreeMap::from([
                ("Add".to_string(), aie("Add", TaskLevel::Lower)),
                ("Mid".to_string(), aie("Mid", TaskLevel::Upper)),
                ("Load".to_string(), task("Load")),
            ]),
        }
    }

    #[test]
    fn flags_parse_their_documented_forms() {
        let pin = parse_aie_location("Add:22,0").expect("a tile");
        assert_eq!(
            pin.location,
            AieLocation {
                col: 22,
                row: 0,
                instance: None
            }
        );
        let pin = parse_aie_location("ns::Add:22,0,3").expect("a tile of an instance");
        assert_eq!(
            (pin.task.as_str(), pin.location.instance),
            ("ns::Add", Some(3))
        );
        let plio = parse_aie_plio("Add.a:64").expect("a width");
        assert_eq!(
            (plio.task.as_str(), plio.port.as_str(), plio.width),
            ("Add", "a", 64)
        );
        for bad in ["Add", "Add:22", "Add:22,0,1,2", ":1,2", "Add:x,0"] {
            assert!(parse_aie_location(bad).is_err(), "{bad}");
        }
        for bad in ["Add:64", "Add.a:48", ".a:64", "Add.:64"] {
            assert!(parse_aie_plio(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn pins_land_on_ai_engine_leaves_and_streams_only() {
        let mut graph = graph();
        let location = parse_aie_location("Add:22,0").expect("tile");
        let plio = parse_aie_plio("Add.a:64").expect("width");
        apply(&mut graph, &[location.clone(), location], &[plio]).expect("valid pins");
        assert_eq!(
            graph.tasks["Add"].aie_location,
            [AieLocation {
                col: 22,
                row: 0,
                instance: None
            }],
            "a repeated pin is recorded once"
        );
        assert_eq!(graph.tasks["Add"].ports[0].aie_plio, Some(64));

        for (location, plio, message) in [
            ("Nope:1,1", None, "names no task `Nope`"),
            ("Load:1,1", None, "not an AI Engine task"),
            ("Mid:1,1", None, "is an upper task"),
            ("Add:1,1", Some("Add.n:64"), "names no single stream port"),
        ] {
            let locations = [parse_aie_location(location).expect("tile")];
            let plios: Vec<_> = plio
                .map(|p| parse_aie_plio(p).expect("width"))
                .into_iter()
                .collect();
            let error = apply(&mut graph.clone(), &locations, &plios).expect_err(message);
            assert!(error.to_string().contains(message), "{error}");
        }
    }
}
