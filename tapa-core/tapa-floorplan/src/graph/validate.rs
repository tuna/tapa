//! Shared validation for every `FloorGraph` build mode: sanitized-name
//! collision rejection, generated-RTL name reservation, memory-binding
//! expectations, and interface-shape checks.

use std::collections::{BTreeMap, HashMap};

use tapa_ir::port::{sanitize_array_name, sanitize_identifier_name};
use tapa_ir::{
    async_mmap_bridge_instance_name, axi_pipeline_instance_name, control_pipeline_instance_name,
    global_controller_instance_name, local_controller_instance_name, AxiEndpoint, ControlChannel,
    TaskGraph,
};

use super::build::find_port;
use crate::graph::floor_graph::{
    CoLocatedInstance, ControlInterface, ExpectedMemoryEndpoint, GraphError, MemoryInterface,
    Vertex, CONTROL_S_AXI_INSTANCE,
};

/// Distinct logical instances that sanitize to one RTL identifier cannot be
/// constrained independently. Runs for every build mode.
pub(super) fn validate_sanitized_instance_names(top: &tapa_ir::Task) -> Result<(), GraphError> {
    let mut sanitized_instances = BTreeMap::<String, String>::new();
    for (definition, instances) in &top.tasks {
        for (instance_index, instance) in instances.iter().enumerate() {
            let canonical = instance
                .canonical_name(definition, instance_index)
                .into_owned();
            let sanitized = sanitize_identifier_name(&canonical);
            if let Some(first) = sanitized_instances.insert(sanitized.clone(), canonical.clone()) {
                if first != canonical {
                    return Err(GraphError::SanitizedNameCollision {
                        first,
                        second: canonical,
                        sanitized,
                    });
                }
            }
        }
    }
    Ok(())
}

/// Reserve the generated distributed-control names against real RTL names.
pub(super) fn validate_control_names(
    top: &tapa_ir::Task,
    memory: &[MemoryInterface],
    control: ControlInterface,
    vertices: &[Vertex],
    co_located: &[CoLocatedInstance],
) -> Result<(), GraphError> {
    let mut occupied = occupied_rtl_names(top, vertices, co_located)?;
    for interface in memory {
        for (channel, _) in interface.channel_widths.enabled_channels() {
            let name = axi_pipeline_instance_name(&interface.endpoint, channel);
            occupied
                .entry(name.clone())
                .or_insert_with(|| OccupiedName {
                    owner: name.clone(),
                    description: format!("AXI pipeline for `{}`", interface.endpoint.top_port),
                });
        }
    }

    reserve_generated_name(
        &mut occupied,
        global_controller_instance_name().to_string(),
        "global controller",
    )?;
    if control.has_s_axi_control {
        reserve_generated_name(
            &mut occupied,
            CONTROL_S_AXI_INSTANCE.to_string(),
            "S-AXI control block",
        )?;
    }
    for (definition, instances) in &top.tasks {
        for (instance_index, instance) in instances.iter().enumerate() {
            let canonical = instance
                .canonical_name(definition, instance_index)
                .into_owned();
            reserve_generated_name(
                &mut occupied,
                local_controller_instance_name(&canonical),
                &format!("local controller for `{canonical}`"),
            )?;
            let channels = [ControlChannel::Launch, ControlChannel::Reset]
                .into_iter()
                .chain((instance.step >= 0).then_some(ControlChannel::Completion));
            for channel in channels {
                reserve_generated_name(
                    &mut occupied,
                    control_pipeline_instance_name(&canonical, channel),
                    &format!("{channel:?} pipeline for `{canonical}`"),
                )?;
            }
        }
    }
    Ok(())
}

pub(super) fn reserve_generated_name(
    occupied: &mut BTreeMap<String, OccupiedName>,
    generated: String,
    owner: &str,
) -> Result<(), GraphError> {
    if let Some(existing) = occupied.get(&generated) {
        return Err(GraphError::GeneratedNameCollision {
            generated,
            existing: existing.description.clone(),
        });
    }
    occupied.insert(
        generated.clone(),
        OccupiedName {
            owner: generated,
            description: owner.to_string(),
        },
    );
    Ok(())
}

/// One claimed name: the logical object that owns it plus a user-facing
/// description for collision diagnostics.
#[derive(Debug, Clone)]
pub(super) struct OccupiedName {
    owner: String,
    description: String,
}

/// Claim `name` for `owner`; the same logical object may occupy several
/// spellings (canonical, sanitized, emitted `{name}_fifo`), but two different
/// owners may never share one name.
fn occupy(
    occupied: &mut BTreeMap<String, OccupiedName>,
    name: String,
    owner: &str,
    description: String,
) -> Result<(), GraphError> {
    match occupied.get(&name) {
        Some(existing) if existing.owner != owner => Err(GraphError::NameConflict {
            name,
            first: existing.description.clone(),
            second: description,
        }),
        Some(_) => Ok(()),
        None => {
            occupied.insert(
                name,
                OccupiedName {
                    owner: owner.to_string(),
                    description,
                },
            );
            Ok(())
        }
    }
}

/// Every name a build claims for placement and XDC matching, or the first
/// collision between two different logical owners.
pub(super) fn occupied_rtl_names(
    top: &tapa_ir::Task,
    vertices: &[Vertex],
    co_located: &[CoLocatedInstance],
) -> Result<BTreeMap<String, OccupiedName>, GraphError> {
    // Owners carry a kind prefix because task, stream, and alias names live
    // in one published namespace: a stream named like a vertex is a conflict,
    // not shared ownership.
    let mut occupied = BTreeMap::new();
    for vertex in vertices {
        let description = || format!("placement vertex `{}`", vertex.name);
        let owner = format!("vertex:{}", vertex.name);
        occupy(&mut occupied, vertex.name.clone(), &owner, description())?;
        occupy(
            &mut occupied,
            sanitize_identifier_name(&vertex.name),
            &owner,
            description(),
        )?;
    }
    for fifo in top.fifos.keys() {
        let description = || format!("stream `{fifo}`");
        let owner = format!("stream:{fifo}");
        occupy(&mut occupied, fifo.clone(), &owner, description())?;
        occupy(
            &mut occupied,
            format!("{}_fifo", sanitize_array_name(fifo)),
            &owner,
            description(),
        )?;
    }
    for alias in co_located {
        // A FIFO's own co-location alias is the same logical object as the
        // stream entry above, never an independent claimant.
        if top.fifos.contains_key(&alias.name) {
            continue;
        }
        let description = || format!("co-located RTL instance `{}`", alias.name);
        let owner = format!("alias:{}", alias.name);
        occupy(&mut occupied, alias.name.clone(), &owner, description())?;
        occupy(
            &mut occupied,
            sanitize_identifier_name(&alias.name),
            &owner,
            description(),
        )?;
    }
    Ok(occupied)
}

pub(super) fn validate_memory_interface_shape(
    interface: &MemoryInterface,
    expected: ExpectedMemoryEndpoint,
) -> Result<(), GraphError> {
    let endpoint = &interface.endpoint;
    let unsupported = |detail: String| GraphError::UnsupportedMemoryInterface {
        port: endpoint.top_port.clone(),
        detail,
    };
    match (expected.child_category, &interface.bridge_instance) {
        (tapa_ir::ArgCategory::Mmap, Some(_)) => {
            return Err(unsupported(format!(
                "plain mmap endpoint `{}.{}` cannot own an async bridge",
                endpoint.instance, endpoint.port
            )));
        }
        (tapa_ir::ArgCategory::AsyncMmap, Some(actual)) => {
            let stable = async_mmap_bridge_instance_name(&endpoint.top_port);
            if *actual != stable {
                return Err(unsupported(format!(
                    "async bridge name `{actual}` does not match stable hierarchy `{stable}`"
                )));
            }
        }
        (tapa_ir::ArgCategory::Mmap | tapa_ir::ArgCategory::AsyncMmap, None) => {}
        _ => unreachable!("expected memory endpoint has a direct mmap category"),
    }

    let widths = interface.channel_widths;
    let read_enabled = (widths.read_address != 0, widths.read_data != 0);
    if read_enabled.0 != read_enabled.1 {
        return Err(unsupported(format!(
            "endpoint `{}.{}` has a partial read channel group",
            endpoint.instance, endpoint.port
        )));
    }
    let write_enabled = (
        widths.write_address != 0,
        widths.write_data != 0,
        widths.write_response != 0,
    );
    if write_enabled.0 != write_enabled.1 || write_enabled.0 != write_enabled.2 {
        return Err(unsupported(format!(
            "endpoint `{}.{}` has a partial write channel group",
            endpoint.instance, endpoint.port
        )));
    }
    if interface.bridge_instance.is_none() && (!read_enabled.0 || !write_enabled.0) {
        return Err(unsupported(format!(
            "direct compact M-AXI endpoint `{}.{}` must expose all five channels",
            endpoint.instance, endpoint.port
        )));
    }
    Ok(())
}

pub(super) fn expected_memory_interfaces(
    flat: &TaskGraph,
    top: &tapa_ir::Task,
    task_endpoints: &HashMap<(String, u32), usize>,
) -> Result<BTreeMap<AxiEndpoint, ExpectedMemoryEndpoint>, GraphError> {
    let mut expected = BTreeMap::new();
    let mut top_port_users = BTreeMap::<String, Vec<String>>::new();

    for (definition, instances) in &top.tasks {
        let task = flat
            .tasks
            .get(definition)
            .ok_or_else(|| GraphError::MissingTaskDef(definition.clone()))?;
        for (instance_index, instance) in instances.iter().enumerate() {
            let instance_name = instance
                .canonical_name(definition, instance_index)
                .into_owned();
            let endpoint_index = u32::try_from(instance_index).expect("instance count fits u32");
            let task_vertex = task_endpoints[&(definition.clone(), endpoint_index)];
            for (child_port, argument) in &instance.args {
                if !argument.cat.is_direct_mmap() {
                    continue;
                }
                let parent_port =
                    argument
                        .name()
                        .ok_or_else(|| GraphError::UnsupportedMemoryInterface {
                            port: child_port.clone(),
                            detail: format!(
                                "child endpoint `{instance_name}.{child_port}` binds a constant, \
                             not a top-level mmap"
                            ),
                        })?;

                let child = find_port(&task.ports, child_port).ok_or_else(|| {
                    GraphError::UnsupportedMemoryInterface {
                        port: parent_port.to_owned(),
                        detail: format!(
                            "child endpoint `{instance_name}.{child_port}` has no port metadata"
                        ),
                    }
                })?;
                let parent = find_port(&top.ports, parent_port).ok_or_else(|| {
                    GraphError::UnsupportedMemoryInterface {
                        port: parent_port.to_owned(),
                        detail: "top-level mmap port metadata is missing".to_string(),
                    }
                })?;
                if child.cat != argument.cat || parent.cat != tapa_ir::ArgCategory::Mmap {
                    return Err(GraphError::UnsupportedMemoryInterface {
                        port: parent_port.to_owned(),
                        detail: format!(
                            "child binding category '{}' must match child metadata '{}' and connect to a plain top mmap",
                            argument.cat.as_str(),
                            child.cat.as_str(),
                        ),
                    });
                }
                if child.chan_count.is_some()
                    || child.chan_size.is_some()
                    || parent.chan_count.is_some()
                    || parent.chan_size.is_some()
                {
                    return Err(GraphError::UnsupportedMemoryInterface {
                        port: parent_port.to_owned(),
                        detail: "hierarchical mmap channels are not modeled".to_string(),
                    });
                }

                let endpoint = AxiEndpoint {
                    instance: instance_name.clone(),
                    port: child_port.clone(),
                    top_port: parent_port.to_owned(),
                };
                top_port_users
                    .entry(parent_port.to_owned())
                    .or_default()
                    .push(format!("{instance_name}.{child_port}"));
                expected.insert(
                    endpoint,
                    ExpectedMemoryEndpoint {
                        task_vertex,
                        child_category: child.cat,
                    },
                );
            }
        }
    }

    if let Some((top_port, users)) = top_port_users
        .into_iter()
        .find(|(_, users)| users.len() > 1)
    {
        return Err(GraphError::UnsupportedMemoryInterface {
            port: top_port,
            detail: format!("shared by {} child endpoints", users.join(", ")),
        });
    }
    Ok(expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::floor_graph::FloorGraph;
    use crate::graph::query::tests::{
        async_mmap_graph, async_mmap_interface, control_memory_interface,
        distributed_control_graph, mmap_graph, vadd_graph,
    };
    use tapa_ir::TaskGraph;

    #[test]
    fn ambiguous_names_are_rejected_without_control_too() {
        let cases = [
            (
                serde_json::json!({
                    "A": [{"name":"same","args":{}}, {"name":"same","args":{}}]
                }),
                "duplicate flattened instance canonical name",
            ),
            (
                serde_json::json!({
                    "A": [{"name":"same#name","args":{}}, {"name":"same?name","args":{}}]
                }),
                "both sanitize to RTL name",
            ),
        ];
        for (tasks, expected) in cases {
            let mut value = serde_json::to_value(vadd_graph()).expect("serialize graph");
            value["tasks"]["VecAdd"]["tasks"] = tasks;
            value["tasks"]["VecAdd"]["fifos"] = serde_json::json!({});
            let design = TaskGraph::from_json(&value.to_string()).expect("parse case");
            for control in [None, Some(ControlInterface::default())] {
                let error = FloorGraph::build_with_interfaces(&design, &[], control, None)
                    .expect_err("ambiguous names must fail in every build mode");
                assert!(
                    error.to_string().contains(expected),
                    "control={control:?} expected `{expected}`, got: {error}"
                );
            }
        }
    }

    #[test]
    fn fifo_and_task_public_key_collision_fails_closed() {
        // Flattening globally names the stream `q` as `q_VecAdd`, which would
        // share one `regions` key with a task explicitly named `q_VecAdd`:
        // the co-location alias would silently overwrite that placement.
        let mut value = serde_json::to_value(vadd_graph()).expect("serialize graph");
        value["tasks"]["VecAdd"]["fifos"] = serde_json::json!({
            "q": {"depth": 2, "consumed_by": ["B", 0], "produced_by": ["A", 0]}
        });
        value["tasks"]["VecAdd"]["tasks"]["A"][0]["args"] =
            serde_json::json!({"out": {"arg": "q", "cat": "ostream"}});
        value["tasks"]["VecAdd"]["tasks"]["B"][0]["name"] =
            serde_json::Value::String("q_VecAdd".to_string());
        value["tasks"]["VecAdd"]["tasks"]["B"][0]["args"] =
            serde_json::json!({"in": {"arg": "q", "cat": "istream"}});
        let design = TaskGraph::from_json(&value.to_string()).expect("parse");
        let flat = tapa_ir::flatten(&design).expect("flatten");
        let error = FloorGraph::build(&flat).expect_err("stream named q_VecAdd must fail");
        assert!(
            matches!(error, GraphError::NameConflict { .. }),
            "got {error}"
        );
    }

    #[test]
    fn fifo_rtl_name_collision_with_task_name_fails_closed() {
        // A task `fifo_VecAdd_fifo` would be claimed by stream `fifo_VecAdd`'s
        // emitted `{sanitized}_fifo` hierarchy.
        let mut value = serde_json::to_value(vadd_graph()).expect("serialize graph");
        value["tasks"]["VecAdd"]["tasks"]["A"][0]["name"] =
            serde_json::Value::String("fifo_VecAdd_fifo".to_string());
        let design = TaskGraph::from_json(&value.to_string()).expect("parse");
        let flat = tapa_ir::flatten(&design).expect("flatten");
        let error = FloorGraph::build(&flat).expect_err("task name fifo_VecAdd_fifo must fail");
        assert!(
            matches!(error, GraphError::NameConflict { .. }),
            "got {error}"
        );
    }

    #[test]
    fn distributed_control_rejects_ambiguous_or_missing_names_and_scalars() {
        let cases = [
            (
                serde_json::json!({
                    "A": [{"name":"same","args":{}}, {"name":"same","args":{}}]
                }),
                "duplicate flattened instance canonical name",
            ),
            (
                serde_json::json!({
                    "A": [{"name":"same#name","args":{}}, {"name":"same?name","args":{}}]
                }),
                "both sanitize to RTL name",
            ),
            (
                serde_json::json!({
                    "A": [{"name":"__tapa_global_controller","args":{}}]
                }),
                "generated RTL name",
            ),
        ];
        for (tasks, expected) in cases {
            let mut value = serde_json::to_value(vadd_graph()).expect("serialize graph");
            value["tasks"]["VecAdd"]["tasks"] = tasks;
            value["tasks"]["VecAdd"]["fifos"] = serde_json::json!({});
            let design = TaskGraph::from_json(&value.to_string()).expect("parse case");
            let error = FloorGraph::build_with_interfaces(
                &design,
                &[],
                Some(ControlInterface::default()),
                None,
            )
            .expect_err("invalid control graph");
            assert!(error.to_string().contains(expected), "{error}");
        }

        let mut value = serde_json::to_value(distributed_control_graph()).expect("serialize graph");
        value["tasks"]["Top"]["tasks"]["Ticker"][0]["args"] = serde_json::json!({});
        let design = TaskGraph::from_json(&value.to_string()).expect("parse missing scalar");
        let flat = tapa_ir::flatten(&design).expect("flatten missing scalar");
        let error = FloorGraph::build_with_interfaces(
            &flat,
            &[control_memory_interface()],
            Some(ControlInterface::default()),
            None,
        )
        .expect_err("missing scalar must fail");
        assert!(matches!(error, GraphError::ScalarMetadata { .. }));
    }

    #[test]
    fn async_bridge_name_collision_fails_closed() {
        let flat = tapa_ir::flatten(&async_mmap_graph("mem__m_axi")).expect("flatten");
        let error = FloorGraph::build_with_memory(
            &flat,
            &[async_mmap_interface("mem__m_axi", true, false)],
        )
        .expect_err("generated bridge must not alias its task");

        assert!(matches!(
            error,
            GraphError::GeneratedNameCollision { ref generated, .. }
                if generated == "mem__m_axi"
        ));
    }

    #[test]
    fn direct_mmap_without_exact_interface_fails_closed() {
        let flat = tapa_ir::flatten(&mmap_graph()).expect("flatten mmap graph");
        let error = FloorGraph::build(&flat).expect_err("binding is required");
        assert!(matches!(
            error,
            GraphError::MissingMemoryInterface {
                ref instance,
                ref port,
                ref top_port,
            } if instance == "Reader_0" && port == "data" && top_port == "mem"
        ));
    }
}
