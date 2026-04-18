//! Interface role inference from port directions.
//!
//! Mirrors Python `tapa/graphir_conversion/pipeline/iface_roles.py`: roles are
//! derived from the direction of the interface's data ports. Handshake and
//! `ap_ctrl` interfaces also validate direction consistency and raise an error
//! if the valid/ready or `ap_start` / `ap_ready` / `ap_done` / `ap_idle` ports disagree.

use std::collections::HashMap;

use tapa_graphir::{interface::AnyInterface, AnyModuleDefinition, ModulePort};

use crate::LoweringError;

/// Role string used on well-formed source-side interfaces.
pub const ROLE_SOURCE: &str = "source";
/// Role string used on well-formed sink-side interfaces.
pub const ROLE_SINK: &str = "sink";

/// Apply a role to every interface of every module that has ports.
///
/// Mirrors Python's `_apply_iface_roles`: for each module with interfaces,
/// walk the interfaces and replace `role` with "source"/"sink" based on
/// port directions. Non-data interfaces (`NonPipeline`, `Unknown`, …) keep
/// their default role.
pub fn apply_iface_roles(
    module_defs: &[AnyModuleDefinition],
    ifaces: &mut std::collections::BTreeMap<String, Vec<AnyInterface>>,
) -> Result<(), LoweringError> {
    let modules_by_name: HashMap<&str, &AnyModuleDefinition> =
        module_defs.iter().map(|m| (m.name(), m)).collect();

    for (module_name, module_ifaces) in ifaces.iter_mut() {
        let Some(module) = modules_by_name.get(module_name.as_str()) else {
            continue;
        };
        let ports_by_name: HashMap<&str, &ModulePort> =
            module.ports().iter().map(|p| (p.name.as_str(), p)).collect();
        for iface in module_ifaces.iter_mut() {
            let role = infer_role(iface, &ports_by_name, module_name)?;
            if let Some(role) = role {
                role.clone_into(&mut iface.base_mut().role);
            }
        }
    }
    Ok(())
}

fn infer_role(
    iface: &AnyInterface,
    ports: &HashMap<&str, &ModulePort>,
    module_name: &str,
) -> Result<Option<&'static str>, LoweringError> {
    match iface {
        AnyInterface::HandShake {
            valid_port,
            ready_port,
            ..
        } => infer_handshake_role(iface, ports, module_name, valid_port.as_ref(), ready_port.as_ref()).map(Some),
        AnyInterface::ApCtrl {
            ap_start_port,
            ap_ready_port,
            ap_done_port,
            ap_idle_port,
            ap_continue_port,
            ..
        } => infer_ap_ctrl_role(
            ports,
            module_name,
            ap_start_port.as_deref(),
            ap_ready_port.as_deref(),
            ap_done_port.as_deref(),
            ap_idle_port.as_deref(),
            ap_continue_port.as_deref(),
        )
        .map(Some),
        AnyInterface::FeedForward { .. }
        | AnyInterface::FeedForwardReset { .. }
        | AnyInterface::FalsePath { .. }
        | AnyInterface::FalsePathReset { .. } => {
            infer_data_role(iface, ports, module_name).map(Some)
        }
        AnyInterface::Clock { .. }
        | AnyInterface::NonPipeline { .. }
        | AnyInterface::Unknown { .. }
        | AnyInterface::TapaPeek { .. }
        | AnyInterface::Aux { .. } => Ok(None),
    }
}

fn infer_handshake_role(
    iface: &AnyInterface,
    ports: &HashMap<&str, &ModulePort>,
    module_name: &str,
    valid_port: Option<&String>,
    ready_port: Option<&String>,
) -> Result<&'static str, LoweringError> {
    let (Some(valid), Some(ready)) = (valid_port.map(String::as_str), ready_port.map(String::as_str)) else {
        return Err(LoweringError::InterfaceDirection(format!(
            "handshake in {module_name} missing valid/ready port"
        )));
    };
    let valid_p = lookup(ports, valid, module_name)?;
    let ready_p = lookup(ports, ready, module_name)?;
    // Data ports: everything on the interface except clk/rst/valid/ready.
    let base = iface.base();
    let exclude: std::collections::HashSet<&str> = [
        base.clk_port.as_deref(),
        base.rst_port.as_deref(),
        Some(valid),
        Some(ready),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut data_ports: Vec<&ModulePort> = Vec::new();
    for name in &base.ports {
        if !exclude.contains(name.as_str()) {
            data_ports.push(lookup(ports, name, module_name)?);
        }
    }

    match (valid_p.is_input(), ready_p.is_output()) {
        (true, true) => {
            if data_ports.iter().any(|p| p.is_output()) {
                return Err(LoweringError::InterfaceDirection(format!(
                    "Incorrect handshake in {module_name}. Data ports should have same direction \
                     as the valid port. The valid port {valid} is input, but some data ports are output."
                )));
            }
            Ok(ROLE_SINK)
        }
        _ => {
            if valid_p.is_output() && ready_p.is_input() {
                if data_ports.iter().any(|p| p.is_input()) {
                    return Err(LoweringError::InterfaceDirection(format!(
                        "Incorrect handshake in {module_name}. Data ports should have same direction \
                         as the valid port. The valid port {valid} is output, but some data ports are input."
                    )));
                }
                Ok(ROLE_SOURCE)
            } else {
                Err(LoweringError::InterfaceDirection(format!(
                    "Incorrect handshake in {module_name}. The valid port {valid} and ready port \
                     {ready} should be of opposite directions."
                )))
            }
        }
    }
}

fn infer_ap_ctrl_role(
    ports: &HashMap<&str, &ModulePort>,
    module_name: &str,
    ap_start: Option<&str>,
    ap_ready: Option<&str>,
    ap_done: Option<&str>,
    ap_idle: Option<&str>,
    ap_continue: Option<&str>,
) -> Result<&'static str, LoweringError> {
    let ap_start = ap_start.ok_or_else(|| {
        LoweringError::InterfaceDirection(format!("ap_ctrl in {module_name} missing ap_start"))
    })?;
    let start_port = lookup(ports, ap_start, module_name)?;

    let validate = |name: Option<&str>, expect_input: bool| -> Result<(), LoweringError> {
        if let Some(n) = name {
            let p = lookup(ports, n, module_name)?;
            let ok = if expect_input { p.is_input() } else { p.is_output() };
            if !ok {
                return Err(LoweringError::InterfaceDirection(format!(
                    "Incorrect ap_ctrl direction in {module_name}: port {n} has wrong direction \
                     relative to ap_start"
                )));
            }
        }
        Ok(())
    };

    if start_port.is_input() {
        validate(ap_ready, false)?;
        validate(ap_done, false)?;
        validate(ap_idle, false)?;
        validate(ap_continue, true)?;
        Ok(ROLE_SINK)
    } else if start_port.is_output() {
        validate(ap_ready, true)?;
        validate(ap_done, true)?;
        validate(ap_idle, true)?;
        validate(ap_continue, false)?;
        Ok(ROLE_SOURCE)
    } else {
        Err(LoweringError::InterfaceDirection(format!(
            "ap_start port {ap_start} in {module_name} has unknown direction"
        )))
    }
}

fn infer_data_role(
    iface: &AnyInterface,
    ports: &HashMap<&str, &ModulePort>,
    module_name: &str,
) -> Result<&'static str, LoweringError> {
    let data = data_port_directions(iface, ports, module_name)?;
    if data.is_empty() {
        // Mirrors Python behavior: if no data ports, default to sink.
        return Ok(ROLE_SINK);
    }
    if data.iter().all(|p| p.is_input()) {
        Ok(ROLE_SINK)
    } else if data.iter().all(|p| p.is_output()) {
        Ok(ROLE_SOURCE)
    } else {
        Err(LoweringError::InterfaceDirection(format!(
            "Mixed directions on {} interface in {module_name}; data ports must all be input or all output",
            iface.type_name(),
        )))
    }
}

fn data_port_directions<'a>(
    iface: &AnyInterface,
    ports: &'a HashMap<&str, &'a ModulePort>,
    module_name: &str,
) -> Result<Vec<&'a ModulePort>, LoweringError> {
    let mut out = Vec::new();
    for name in iface.data_ports() {
        out.push(lookup(ports, &name, module_name)?);
    }
    Ok(out)
}

fn lookup<'a>(
    ports: &'a HashMap<&str, &'a ModulePort>,
    name: &str,
    module_name: &str,
) -> Result<&'a ModulePort, LoweringError> {
    ports.get(name).copied().ok_or_else(|| {
        LoweringError::InterfaceDirection(format!(
            "port {name} referenced by interface of {module_name} not found on the module"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tapa_graphir::{interface::InterfaceBase, BaseFields, VerilogFields};

    fn mk_module(name: &str, ports: Vec<ModulePort>) -> AnyModuleDefinition {
        AnyModuleDefinition::Verilog {
            base: BaseFields {
                name: name.into(),
                hierarchical_name: tapa_graphir::HierarchicalName::none(),
                parameters: Vec::new(),
                ports,
                metadata: None,
            },
            verilog: VerilogFields {
                verilog: String::new(),
                submodules_module_names: Vec::new(),
            },
            extra: BTreeMap::default(),
        }
    }

    fn mk_handshake(valid: &str, ready: &str, ports: Vec<String>) -> AnyInterface {
        AnyInterface::HandShake {
            base: InterfaceBase {
                clk_port: Some("clk".into()),
                rst_port: Some("rst".into()),
                ports,
                role: String::new(),
                origin_info: String::new(),
            },
            valid_port: Some(valid.into()),
            ready_port: Some(ready.into()),
            data_ports: Vec::new(),
            extra: BTreeMap::default(),
        }
    }

    fn input(name: &str) -> ModulePort {
        ModulePort {
            name: name.into(),
            hierarchical_name: tapa_graphir::HierarchicalName::get_name(name),
            port_type: "input wire".into(),
            range: None,
            extra: BTreeMap::default(),
        }
    }

    fn output(name: &str) -> ModulePort {
        ModulePort {
            name: name.into(),
            hierarchical_name: tapa_graphir::HierarchicalName::get_name(name),
            port_type: "output wire".into(),
            range: None,
            extra: BTreeMap::default(),
        }
    }

    #[test]
    fn handshake_sink_gets_sink_role() {
        let module = mk_module(
            "m",
            vec![input("clk"), input("rst"), input("valid"), output("ready"), input("data")],
        );
        let mut ifaces = std::collections::BTreeMap::from([(
            "m".to_owned(),
            vec![mk_handshake(
                "valid",
                "ready",
                vec![
                    "clk".into(),
                    "rst".into(),
                    "valid".into(),
                    "ready".into(),
                    "data".into(),
                ],
            )],
        )]);
        apply_iface_roles(&[module], &mut ifaces).expect("should succeed");
        assert_eq!(ifaces["m"][0].base().role, "sink");
    }

    #[test]
    fn handshake_mixed_direction_fails() {
        let module = mk_module(
            "m",
            vec![input("clk"), input("rst"), input("valid"), output("ready"), output("data")],
        );
        let mut ifaces = std::collections::BTreeMap::from([(
            "m".to_owned(),
            vec![mk_handshake(
                "valid",
                "ready",
                vec![
                    "clk".into(),
                    "rst".into(),
                    "valid".into(),
                    "ready".into(),
                    "data".into(),
                ],
            )],
        )]);
        let err = apply_iface_roles(&[module], &mut ifaces)
            .expect_err("mixed directions should fail");
        assert!(err.to_string().contains("data ports"), "got: {err}");
    }

    #[test]
    fn handshake_source_gets_source_role() {
        let module = mk_module(
            "m",
            vec![input("clk"), input("rst"), output("valid"), input("ready"), output("data")],
        );
        let mut ifaces = std::collections::BTreeMap::from([(
            "m".to_owned(),
            vec![mk_handshake(
                "valid",
                "ready",
                vec![
                    "clk".into(),
                    "rst".into(),
                    "valid".into(),
                    "ready".into(),
                    "data".into(),
                ],
            )],
        )]);
        apply_iface_roles(&[module], &mut ifaces).expect("should succeed");
        assert_eq!(ifaces["m"][0].base().role, "source");
    }

    #[test]
    fn handshake_same_direction_fails() {
        let module = mk_module(
            "m",
            vec![input("clk"), input("rst"), input("valid"), input("ready")],
        );
        let mut ifaces = std::collections::BTreeMap::from([(
            "m".to_owned(),
            vec![mk_handshake(
                "valid",
                "ready",
                vec!["clk".into(), "rst".into(), "valid".into(), "ready".into()],
            )],
        )]);
        let err = apply_iface_roles(&[module], &mut ifaces)
            .expect_err("invalid directions should fail");
        assert!(err.to_string().contains("opposite directions"), "got: {err}");
    }
}
