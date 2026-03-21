"""Interface role inference helpers for GraphIR conversion."""

from __future__ import annotations

from tapa.graphir.types import (
    AnyInterface,
    AnyModuleDefinition,
    ApCtrlInterface,
    BaseInterface,
    FalsePathInterface,
    FeedForwardInterface,
    HandShakeInterface,
    ModulePort,
    NonPipelineInterface,
)


def set_iface_role(module: AnyModuleDefinition, iface: AnyInterface) -> AnyInterface:
    """Set the role of the interface based on port directions."""
    name_to_port = {p.name: p for p in module.ports}
    return _set_iface_role_by_ports(name_to_port, iface, module.name)


def _set_as_sink(iface: AnyInterface) -> AnyInterface:
    """Return a copy of the interface whose role is set as sink."""
    return iface.updated(role=BaseInterface.InterfaceRole.SINK)


def _set_as_source(iface: AnyInterface) -> AnyInterface:
    """Return a copy of the interface whose role is set as source."""
    return iface.updated(role=BaseInterface.InterfaceRole.SOURCE)


def _set_hs_iface_role_by_ports(
    name_to_port: dict[str, ModulePort],
    iface: HandShakeInterface,
    module_name: str,
) -> AnyInterface:
    """Set the role of the handshake interface based on port directions."""
    valid_port = name_to_port[iface.valid_port]
    ready_port = name_to_port[iface.ready_port]
    data_ports = [name_to_port[p] for p in iface.get_data_ports()]

    input_data_ports = {p.name for p in data_ports if p.is_input_port()}
    output_data_ports = {p.name for p in data_ports if p.is_output_port()}

    if valid_port.is_input_port() and ready_port.is_output_port():
        if output_data_ports:
            msg = (
                f"Incorrect handshake in {module_name}. Data ports should have same "
                f"direction to the valid port. The valid port {valid_port} is input, "
                f"but these data ports are output {output_data_ports}: {iface}"
            )
            raise ValueError(msg)
        return _set_as_sink(iface)

    if valid_port.is_output_port() and ready_port.is_input_port():
        if input_data_ports:
            msg = (
                f"Incorrect handshake in {module_name}. Data ports should have same "
                f"direction to the valid port. The valid port {valid_port} is output, "
                f"but these data ports are input {input_data_ports}: {iface}"
            )
            raise ValueError(msg)
        return _set_as_source(iface)

    msg = (
        f"Incorrect handshake in {module_name}. The valid port {valid_port} and "
        f"ready port {ready_port} should be of opposite directions: {iface}"
    )
    raise ValueError(msg)


def _set_ap_ctrl_iface_role_by_ports(
    name_to_port: dict[str, ModulePort],
    iface: ApCtrlInterface,
    module_name: str,
) -> AnyInterface:
    """Set the role of the ap_ctrl interface based on port directions."""
    ap_start_port = name_to_port[iface.ap_start_port]
    ap_ready_port = name_to_port[iface.ap_ready_port] if iface.ap_ready_port else None
    ap_done_port = name_to_port[iface.ap_done_port] if iface.ap_done_port else None
    ap_idle_port = name_to_port[iface.ap_idle_port] if iface.ap_idle_port else None
    ap_continue_port = (
        name_to_port[iface.ap_continue_port] if iface.ap_continue_port else None
    )

    if ap_start_port.is_input_port():
        if not (
            (ap_ready_port is None or ap_ready_port.is_output_port())
            and (ap_done_port is None or ap_done_port.is_output_port())
            and (ap_idle_port is None or ap_idle_port.is_output_port())
            and (ap_continue_port is None or ap_continue_port.is_input_port())
        ):
            msg = (
                f"Incorrect ap_ctrl port direction in {module_name}. When the "
                f"ap_start port is input, the ap_ready, ap_done, and ap_idle ports "
                f"should be the output ports, and the ap_continue ports should be "
                f"the input ports. {iface}"
            )
            raise ValueError(msg)
        return _set_as_sink(iface)

    if ap_start_port.is_output_port():
        if not (
            (ap_ready_port is None or ap_ready_port.is_input_port())
            and (ap_done_port is None or ap_done_port.is_input_port())
            and (ap_idle_port is None or ap_idle_port.is_input_port())
            and (ap_continue_port is None or ap_continue_port.is_output_port())
        ):
            msg = (
                f"Incorrect ap_ctrl port direction in {module_name}. When the "
                f"ap_start port is output, the ap_ready, ap_done, and ap_idle ports "
                f"should be the input ports, and the ap_continue ports should be "
                f"the output ports. {iface}"
            )
            raise ValueError(msg)
        return _set_as_source(iface)

    msg = f"Unknown ap_start port direction {type(iface)}."
    raise NotImplementedError(msg)


def _set_iface_role_by_ports(
    name_to_port: dict[str, ModulePort],
    iface: AnyInterface,
    module_name: str,
) -> AnyInterface:
    """Set the role of the interface based on port directions."""
    if isinstance(iface, HandShakeInterface):
        return _set_hs_iface_role_by_ports(name_to_port, iface, module_name)

    if isinstance(iface, ApCtrlInterface):
        return _set_ap_ctrl_iface_role_by_ports(name_to_port, iface, module_name)

    if isinstance(iface, (FeedForwardInterface, FalsePathInterface)):
        data_ports = [name_to_port[p] for p in iface.get_data_ports()]
        if all(p.is_input_port() for p in data_ports):
            return _set_as_sink(iface)
        if all(p.is_output_port() for p in data_ports):
            return _set_as_source(iface)
        msg = (
            f"Incorrect {iface.type} interface in {module_name}. The ports should "
            f"be of the same direction. {iface}"
        )
        raise ValueError(msg)

    if isinstance(iface, NonPipelineInterface):
        return iface

    msg = f"Cannot infer role of {type(iface)}."
    raise NotImplementedError(msg)
