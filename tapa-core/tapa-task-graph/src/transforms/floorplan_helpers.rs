//! Internal helpers for [`super::apply_floorplan`].
//!
//! Split out of `transforms.rs` to keep that file under the workspace's
//! 450 LOC ceiling. None of these are part of the public API.

use std::collections::{BTreeMap, BTreeSet};

use crate::instance::{Arg, TaskInstance};
use crate::interconnect::{EndpointRef, InterconnectDefinition};
use crate::port::{ArgCategory, Port};
use crate::task::{TaskDefinition, TaskLevel};

use super::TransformError;

pub(super) fn rewritten_endpoint_idx(
    endpoint: &EndpointRef,
    slot_idx_map: &BTreeMap<String, BTreeMap<usize, usize>>,
) -> Option<EndpointRef> {
    let EndpointRef(task, top_idx) = endpoint;
    let top_idx_usize = *top_idx as usize;
    let new_idx = slot_idx_map.get(task)?.get(&top_idx_usize)?;
    let new_idx_u32 = u32::try_from(*new_idx).unwrap_or(u32::MAX);
    Some(EndpointRef(task.clone(), new_idx_u32))
}

pub(super) fn build_slot_def(
    top_def: &TaskDefinition,
    top_name: &str,
    slot_name: &str,
    slot_insts: &[String],
    inst_name_to_pos: &BTreeMap<String, (String, usize)>,
    all_tasks: &BTreeMap<String, TaskDefinition>,
) -> Result<TaskDefinition, TransformError> {
    let mut slot_tasks: BTreeMap<String, Vec<TaskInstance>> = BTreeMap::new();
    let mut slot_idx_map: BTreeMap<String, BTreeMap<usize, usize>> = BTreeMap::new();

    for inst_name in slot_insts {
        let (def_name, top_idx) = inst_name_to_pos
            .get(inst_name)
            .ok_or_else(|| TransformError::UnknownFloorplanInstance(inst_name.clone()))?;
        let original = top_def
            .tasks
            .get(def_name)
            .and_then(|v| v.get(*top_idx))
            .ok_or_else(|| TransformError::UnknownFloorplanInstance(inst_name.clone()))?;
        let entry = slot_tasks.entry(def_name.clone()).or_default();
        let new_idx = entry.len();
        entry.push(connect_subinst_mmap_to_slot_port(original, inst_name));
        slot_idx_map
            .entry(def_name.clone())
            .or_default()
            .insert(*top_idx, new_idx);
    }

    let (slot_fifos, fifo_ports) = compute_slot_fifos(top_def, slot_insts, &slot_idx_map);

    let scalar_args: BTreeSet<String> = slot_tasks
        .values()
        .flatten()
        .flat_map(|inst| inst.args.values())
        .filter(|a| a.cat == ArgCategory::Scalar)
        .map(|a| a.arg.clone())
        .collect();

    let mut new_ports: Vec<Port> = top_def
        .ports
        .iter()
        .filter(|p| scalar_args.contains(&p.name))
        .cloned()
        .collect();
    new_ports.extend(get_used_ports(
        top_def,
        slot_insts,
        &fifo_ports,
        inst_name_to_pos,
        all_tasks,
    )?);
    new_ports.extend(infer_mmap_ports_from_subtasks(
        slot_insts,
        inst_name_to_pos,
        top_def,
        all_tasks,
    )?);

    let slot_ports: Vec<tapa_slotting::SlotPort> = new_ports
        .iter()
        .map(|p| tapa_slotting::SlotPort {
            cat: arg_category_str(p.cat).to_owned(),
            name: p.name.clone(),
            port_type: p.ctype.clone(),
        })
        .collect();
    let new_code = tapa_slotting::gen_slot_cpp(slot_name, top_name, &slot_ports, &top_def.code)
        .map_err(|source| TransformError::SlotCppGeneration {
            slot: slot_name.to_owned(),
            source,
        })?;

    Ok(TaskDefinition {
        code: new_code,
        level: TaskLevel::Upper,
        target: top_def.target.clone(),
        vendor: top_def.vendor.clone(),
        ports: new_ports,
        tasks: slot_tasks,
        fifos: slot_fifos,
    })
}

fn arg_category_str(cat: ArgCategory) -> &'static str {
    match cat {
        ArgCategory::Istream => "istream",
        ArgCategory::Ostream => "ostream",
        ArgCategory::Istreams => "istreams",
        ArgCategory::Ostreams => "ostreams",
        ArgCategory::Scalar => "scalar",
        ArgCategory::Mmap => "mmap",
        ArgCategory::Immap => "immap",
        ArgCategory::Ommap => "ommap",
        ArgCategory::AsyncMmap => "async_mmap",
    }
}

pub(super) fn compute_slot_fifos(
    top_def: &TaskDefinition,
    slot_insts: &[String],
    slot_idx_map: &BTreeMap<String, BTreeMap<usize, usize>>,
) -> (BTreeMap<String, InterconnectDefinition>, Vec<String>) {
    let in_slot: BTreeSet<String> = slot_insts.iter().cloned().collect();
    let mut new_fifos: BTreeMap<String, InterconnectDefinition> = BTreeMap::new();
    let mut fifo_ports: Vec<String> = Vec::new();
    for (fifo_name, fifo) in &top_def.fifos {
        if fifo.depth.is_none() {
            continue;
        }
        let consumer = fifo.consumed_by.as_ref();
        let producer = fifo.produced_by.as_ref();
        let src_in = consumer
            .is_some_and(|EndpointRef(n, i)| in_slot.contains(&endpoint_inst_name(n, *i)));
        let dst_in = producer
            .is_some_and(|EndpointRef(n, i)| in_slot.contains(&endpoint_inst_name(n, *i)));

        let mut maybe_new = None;
        if src_in && dst_in {
            maybe_new = Some(fifo.clone());
        } else if src_in {
            maybe_new = Some(InterconnectDefinition {
                depth: None,
                consumed_by: consumer.cloned(),
                produced_by: None,
            });
            fifo_ports.push(fifo_name.clone());
        } else if dst_in {
            maybe_new = Some(InterconnectDefinition {
                depth: None,
                consumed_by: None,
                produced_by: producer.cloned(),
            });
            fifo_ports.push(fifo_name.clone());
        }
        if let Some(mut nf) = maybe_new {
            if let Some(c) = nf.consumed_by.as_ref() {
                nf.consumed_by = rewritten_endpoint_idx(c, slot_idx_map);
            }
            if let Some(p) = nf.produced_by.as_ref() {
                nf.produced_by = rewritten_endpoint_idx(p, slot_idx_map);
            }
            new_fifos.insert(fifo_name.clone(), nf);
        }
    }
    (new_fifos, fifo_ports)
}

pub(super) fn endpoint_inst_name(task_name: &str, idx: u32) -> String {
    format!("{task_name}_{idx}")
}

pub(super) fn connect_subinst_mmap_to_slot_port(
    inst: &TaskInstance,
    inst_name: &str,
) -> TaskInstance {
    let mut new_args = inst.args.clone();
    for (port_name, arg) in &inst.args {
        if matches!(arg.cat, ArgCategory::Mmap | ArgCategory::AsyncMmap) {
            new_args.insert(
                port_name.clone(),
                Arg {
                    arg: format!("{port_name}_{inst_name}"),
                    cat: arg.cat,
                },
            );
        }
    }
    TaskInstance {
        args: new_args,
        step: inst.step,
    }
}

pub(super) fn get_used_ports(
    top_def: &TaskDefinition,
    slot_insts: &[String],
    fifo_ports: &[String],
    inst_name_to_pos: &BTreeMap<String, (String, usize)>,
    all_tasks: &BTreeMap<String, TaskDefinition>,
) -> Result<Vec<Port>, TransformError> {
    let mut out: Vec<Port> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for inst_name in slot_insts {
        let Some((def_name, idx)) = inst_name_to_pos.get(inst_name) else {
            continue;
        };
        let Some(inst) = top_def.tasks.get(def_name).and_then(|v| v.get(*idx)) else {
            continue;
        };
        let child_def = all_tasks
            .get(def_name)
            .ok_or_else(|| TransformError::UnknownChildTask(def_name.clone()))?;
        for (port_name, arg) in &inst.args {
            if matches!(arg.cat, ArgCategory::Mmap | ArgCategory::AsyncMmap) {
                continue;
            }
            if !fifo_ports.iter().any(|f| f == &arg.arg) {
                continue;
            }
            if !seen.insert(arg.arg.clone()) {
                continue;
            }
            let port_name_no_idx = strip_trailing_index(port_name);
            let child_port = child_def
                .ports
                .iter()
                .find(|p| p.name == port_name_no_idx);
            let (ctype, width, chan_count, chan_size) = child_port.map_or_else(
                || (String::new(), 0, None, None),
                |p| (p.ctype.clone(), p.width, p.chan_count, p.chan_size),
            );
            out.push(Port {
                cat: arg.cat,
                name: arg.arg.clone(),
                ctype,
                width,
                chan_count,
                chan_size,
            });
        }
    }
    Ok(out)
}

pub(super) fn infer_mmap_ports_from_subtasks(
    slot_insts: &[String],
    inst_name_to_pos: &BTreeMap<String, (String, usize)>,
    top_def: &TaskDefinition,
    all_tasks: &BTreeMap<String, TaskDefinition>,
) -> Result<Vec<Port>, TransformError> {
    let mut out: Vec<Port> = Vec::new();
    for inst_name in slot_insts {
        let Some((def_name, idx)) = inst_name_to_pos.get(inst_name) else {
            continue;
        };
        let Some(inst) = top_def.tasks.get(def_name).and_then(|v| v.get(*idx)) else {
            continue;
        };
        let child_def = all_tasks
            .get(def_name)
            .ok_or_else(|| TransformError::UnknownChildTask(def_name.clone()))?;
        for (port_name, arg) in &inst.args {
            if matches!(arg.cat, ArgCategory::Mmap | ArgCategory::AsyncMmap) {
                let child_port = child_def.ports.iter().find(|p| p.name == *port_name);
                let (ctype, width, chan_count, chan_size) = child_port.map_or_else(
                    || (String::new(), 0, None, None),
                    |p| (p.ctype.clone(), p.width, p.chan_count, p.chan_size),
                );
                out.push(Port {
                    cat: arg.cat,
                    name: format!("{port_name}_{inst_name}"),
                    ctype,
                    width,
                    chan_count,
                    chan_size,
                });
            }
        }
    }
    Ok(out)
}

/// Strip a trailing `[index]` suffix from a port name (mirrors Python's
/// `re.sub(r"\[[^\]]+\]$", "", port_name)` used in `_get_used_ports`).
fn strip_trailing_index(name: &str) -> String {
    if let Some(bracket) = name.rfind('[') {
        if name.ends_with(']') {
            return name[..bracket].to_owned();
        }
    }
    name.to_owned()
}

pub(super) fn build_top_slot_instantiations(
    slot_defs: &BTreeMap<String, TaskDefinition>,
    top_def: &TaskDefinition,
    inst_to_slot: &BTreeMap<String, String>,
) -> BTreeMap<String, Vec<TaskInstance>> {
    let mut new_top: BTreeMap<String, Vec<TaskInstance>> = BTreeMap::new();
    for (slot_name, slot_def) in slot_defs {
        let mut args: BTreeMap<String, Arg> = BTreeMap::new();
        for port in &slot_def.ports {
            if matches!(port.cat, ArgCategory::Mmap | ArgCategory::AsyncMmap) {
                continue;
            }
            let port_name_formatted = format_array_suffix(&port.name);
            let cat = infer_arg_cat_from_subinst(&port.name, &slot_def.tasks)
                .unwrap_or(port.cat);
            args.insert(
                port_name_formatted,
                Arg {
                    arg: port.name.clone(),
                    cat,
                },
            );
        }
        for (port_name, arg) in
            slot_inst_mmap_port_args(slot_name, &top_def.tasks, inst_to_slot)
        {
            args.insert(port_name, arg);
        }
        new_top.insert(
            slot_name.clone(),
            vec![TaskInstance { args, step: 0 }],
        );
    }
    new_top
}

fn format_array_suffix(name: &str) -> String {
    if let Some(bracket) = name.rfind('[') {
        if name.ends_with(']') {
            let inside = &name[bracket + 1..name.len() - 1];
            return format!("{}_{}", &name[..bracket], inside);
        }
    }
    name.to_string()
}

fn infer_arg_cat_from_subinst(
    port_name: &str,
    tasks: &BTreeMap<String, Vec<TaskInstance>>,
) -> Option<ArgCategory> {
    for insts in tasks.values() {
        for inst in insts {
            for arg in inst.args.values() {
                if arg.arg == port_name {
                    return Some(arg.cat);
                }
            }
        }
    }
    None
}

fn slot_inst_mmap_port_args(
    slot_name: &str,
    top_tasks: &BTreeMap<String, Vec<TaskInstance>>,
    inst_to_slot: &BTreeMap<String, String>,
) -> Vec<(String, Arg)> {
    let mut out: Vec<(String, Arg)> = Vec::new();
    for (task_name, insts) in top_tasks {
        for (idx, inst) in insts.iter().enumerate() {
            let inst_name = format!("{task_name}_{idx}");
            if inst_to_slot.get(&inst_name).map(String::as_str) != Some(slot_name) {
                continue;
            }
            for (port_name, arg) in &inst.args {
                if matches!(arg.cat, ArgCategory::Mmap | ArgCategory::AsyncMmap) {
                    out.push((
                        format!("{port_name}_{inst_name}"),
                        Arg {
                            arg: arg.arg.clone(),
                            cat: arg.cat,
                        },
                    ));
                }
            }
        }
    }
    out
}

pub(super) fn update_cross_slot_fifos(
    top_fifos: &BTreeMap<String, InterconnectDefinition>,
    in_slot_fifos: &BTreeSet<String>,
    inst_to_slot: &BTreeMap<String, String>,
) -> BTreeMap<String, InterconnectDefinition> {
    let mut out: BTreeMap<String, InterconnectDefinition> = BTreeMap::new();
    for (name, fifo) in top_fifos {
        if in_slot_fifos.contains(name) {
            continue;
        }
        let mut new_fifo = fifo.clone();
        if let Some(EndpointRef(t, i)) = fifo.consumed_by.as_ref() {
            let inst_name = format!("{t}_{i}");
            if let Some(slot) = inst_to_slot.get(&inst_name) {
                new_fifo.consumed_by = Some(EndpointRef(slot.clone(), 0));
            }
        }
        if let Some(EndpointRef(t, i)) = fifo.produced_by.as_ref() {
            let inst_name = format!("{t}_{i}");
            if let Some(slot) = inst_to_slot.get(&inst_name) {
                new_fifo.produced_by = Some(EndpointRef(slot.clone(), 0));
            }
        }
        out.insert(name.clone(), new_fifo);
    }
    out
}
