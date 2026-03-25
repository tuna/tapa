"""Generates an AXI crossbar wrapper with the specified number of ports."""

from pathlib import Path

from jinja2 import Environment, FileSystemLoader, StrictUndefined

_env = Environment(
    loader=FileSystemLoader(str(Path(__file__).parent / "assets")),
    undefined=StrictUndefined,
    trim_blocks=True,
    lstrip_blocks=True,
)
_AXI_PORT_COUNT = 2


def generate(
    ports: int | list[int] | tuple[int, int] = 4, name: str | None = None
) -> str:
    """Generates an AXI crossbar wrapper with the specified number of ports."""
    if isinstance(ports, int):
        m = n = ports
    elif isinstance(ports, (tuple, list)) and len(ports) in {1, 2}:
        m = n = ports[0]
        if len(ports) == _AXI_PORT_COUNT:
            n = ports[1]
    else:
        msg = "Invalid number of ports"
        raise ValueError(msg)

    if name is None:
        name = f"axi_crossbar_wrap_{m}x{n}"

    return _env.get_template("axi_xbar.v.j2").render(m=m, n=n, name=name)
