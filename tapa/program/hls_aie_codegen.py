"""AIE graph code generation helpers for TAPA HLS programs."""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from tapa.task import Task

# _AIE_DEPTH is the number of elements in the AIE windows.
_AIE_DEPTH = 64


def gen_declarations(task: Task) -> tuple[list[str], list[str], list[str]]:
    """Generates kernel and port declarations."""
    port_decl = [
        f"{'input' if port.is_immap or port.is_istream else 'output'}"
        f"_plio p_{port.name};"
        for port in task.ports.values()
    ]
    kernel_decl = [
        f"kernel k_{name}{i};"
        for name, insts in task.tasks.items()
        for i in range(len(insts))
    ]
    return [], kernel_decl, port_decl


def gen_definitions(
    task: Task,
) -> tuple[list[str], list[str], list[str], list[str], list[str]]:
    """Generates kernel and port definitions."""
    kernels = [
        (name, i) for name, insts in task.tasks.items() for i in range(len(insts))
    ]
    kernel_def = [f"k_{n}{i} = kernel::create({n});" for n, i in kernels]
    kernel_source = [f'source(k_{n}{i}) = "../../cpp/{n}.cpp";' for n, i in kernels]
    kernel_runtime = [f"runtime<ratio>(k_{n}{i}) = OCCUPANCY;" for n, i in kernels]
    kernel_loc = [f"//location<kernel>(k_{n}{i}) = tile(X, X);" for n, i in kernels]
    port_def = [
        (
            f'p_{port.name} = input_plio::create("{port.name}",'
            f' plio_{port.width}_bits, "{port.name}.txt");'
            if port.is_immap or port.is_istream
            else f'p_{port.name} = output_plio::create("{port.name}",'
            f' plio_{port.width}_bits, "{port.name}.txt");'
        )
        for port in task.ports.values()
    ]
    return kernel_def, kernel_source, kernel_runtime, kernel_loc, port_def


def _apply_istream_connection(
    conn_dict: dict,
    name: str,
    inst_idx: int,
    in_num: int,
    link_to_dst: dict,
) -> int:
    """Record an istream connection and return the updated in_num."""
    link_to_dst[conn_dict["arg"]] = [f"{name}{inst_idx}.in[{in_num}]", "net"]
    return in_num + 1


def _apply_ostream_connection(
    conn_dict: dict,
    name: str,
    inst_idx: int,
    out_num: int,
    link_from_src: dict,
) -> int:
    """Record an ostream connection and return the updated out_num."""
    link_from_src[conn_dict["arg"]] = [f"{name}{inst_idx}.out[{out_num}]", "net"]
    return out_num + 1


def _apply_immap_connection(
    conn_dict: dict,
    name: str,
    inst_idx: int,
    in_num: int,
    link_to_dst: dict,
) -> int:
    """Record an immap connection and return the updated in_num."""
    link_to_dst[conn_dict["arg"]] = [f"{name}{inst_idx}.in[{in_num}]", "io"]
    return in_num + 1


def _apply_ommap_connection(
    conn_dict: dict,
    name: str,
    inst_idx: int,
    out_num: int,
    link_from_src: dict,
) -> int:
    """Record an ommap connection and return the updated out_num."""
    link_from_src[conn_dict["arg"]] = [f"{name}{inst_idx}.out[{out_num}]", "io"]
    return out_num + 1


def _build_kernel_links(
    task: Task,
) -> tuple[dict[str, list], dict[str, list]]:
    """Build link_from_src and link_to_dst maps from task kernel instances."""
    link_from_src: dict[str, list] = {}
    link_to_dst: dict[str, list] = {}
    for name, insts in task.tasks.items():
        for i, inst in enumerate(insts):
            in_num = 0
            out_num = 0
            for conn_dict in inst["args"].values():
                cat = conn_dict["cat"]
                if cat == "istream":
                    in_num = _apply_istream_connection(
                        conn_dict, name, i, in_num, link_to_dst
                    )
                elif cat == "ostream":
                    out_num = _apply_ostream_connection(
                        conn_dict, name, i, out_num, link_from_src
                    )
                elif cat == "immap":
                    in_num = _apply_immap_connection(
                        conn_dict, name, i, in_num, link_to_dst
                    )
                elif cat == "ommap":
                    out_num = _apply_ommap_connection(
                        conn_dict, name, i, out_num, link_from_src
                    )
                else:
                    msg = f"Unknown connection category: {cat}"
                    raise ValueError(msg)
    return link_from_src, link_to_dst


def _build_fifo_connect_defs(
    task: Task,
    link_from_src: dict[str, list],
    link_to_dst: dict[str, list],
) -> list[str]:
    """Generate connect<stream> lines for matched FIFOs."""
    return [
        f"connect<stream> {name} (k_{link_from_src[name][0]},"
        f" k_{link_to_dst[name][0]});"
        for name in task.fifos
        if name in link_from_src and name in link_to_dst
    ]


def _build_port_connect_defs(
    task: Task,
    link_from_src: dict[str, list],
    link_to_dst: dict[str, list],
) -> list[str]:
    """Generate connect lines for top-level ports."""
    connect_def: list[str] = []
    for port in task.ports.values():
        name = port.name
        width = port.width
        window_default_size = int(width) // 8 * _AIE_DEPTH
        if name in link_from_src:
            conn_type = link_from_src[name][1]
            if conn_type == "io":
                connect_def.append(
                    f"connect<window<{window_default_size}>> {name}_link"
                    f" (k_{link_from_src[name][0]}, p_{name}.in[0]);"
                )
            elif conn_type == "net":
                connect_def.append(
                    f"connect<stream> {name}_link"
                    f" (k_{link_from_src[name][0]}, p_{name}.in[0]);"
                )
            else:
                msg = f"Port[{name}] should be connected to io/net"
                raise ValueError(msg)
        if name in link_to_dst:
            conn_type = link_to_dst[name][1]
            if conn_type == "io":
                connect_def.append(
                    f"connect<window<{window_default_size}>> {name}_link"
                    f" (p_{name}.out[0], k_{link_to_dst[name][0]});"
                )
            elif conn_type == "net":
                connect_def.append(
                    f"connect<stream> {name}_link"
                    f" (p_{name}.out[0], k_{link_to_dst[name][0]});"
                )
            else:
                msg = f"Port[{name}] should be connected to io"
                raise ValueError(msg)
    return connect_def


def gen_connections(task: Task) -> list[str]:
    """Generates connections between ports and kernels."""
    link_from_src, link_to_dst = _build_kernel_links(task)
    connect_def = _build_fifo_connect_defs(task, link_from_src, link_to_dst)
    connect_def.extend(_build_port_connect_defs(task, link_from_src, link_to_dst))
    return connect_def
