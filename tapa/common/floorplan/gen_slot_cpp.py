"""Generate floorplan slot cpp for hls synth."""

__copyright__ = """
Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

import re
from pathlib import Path

import tree_sitter_cpp as tscpp
from jinja2 import Environment, FileSystemLoader
from tree_sitter import Language, Node, Parser, Query, QueryCursor

_CPP_LANGUAGE = Language(tscpp.language())
_parser = Parser(_CPP_LANGUAGE)

_ASSETS_DIR = Path(__file__).with_name("assets")
_env = Environment(loader=FileSystemLoader(_ASSETS_DIR), keep_trailing_newline=True)

_SCALAR_PRAGMA = (
    "#pragma HLS interface ap_none port = {name} register\n"
    "  {{ auto val = reinterpret_cast<volatile uint8_t &>({name}); }}"
)
_MMAP_PRAGMA = (
    "#pragma HLS interface ap_none port = {name}_offset register\n"
    "  {{ auto val = reinterpret_cast<volatile uint8_t &>({name}_offset); }}"
)
_FIFO_IN_PRAGMA = (
    "#pragma HLS disaggregate variable = {name}\n"
    "#pragma HLS interface ap_fifo port = {name}._\n"
    "#pragma HLS aggregate variable = {name}._ bit\n"
    "  void({name}._.empty());\n"
    "  {{ auto val = {name}.read(); }}\n"
)
_FIFO_OUT_PRAGMA = (
    "#pragma HLS disaggregate variable = {name}\n"
    "#pragma HLS interface ap_fifo port = {name}._\n"
    "#pragma HLS aggregate variable = {name}._ bit\n"
    "  void({name}._.full());\n"
    "  {name}.write({type}());"
)

_PRAGMA = {
    "scalar": _SCALAR_PRAGMA,
    "async_mmap": _MMAP_PRAGMA,
    "mmap": _MMAP_PRAGMA,
    "hmap": _SCALAR_PRAGMA,
    "istream": _FIFO_IN_PRAGMA,
    "ostream": _FIFO_OUT_PRAGMA,
    "istreams": _FIFO_IN_PRAGMA,
    "ostreams": _FIFO_OUT_PRAGMA,
}

_SCALAR_PORT_TEMPLATE = "{type} {name}"
_MMAP_PORT_TEMPLATE = "{type} {name}_offset"
_PORT_TEMPLATE = {
    "istream": "tapa::istream<{type}>& {name}",
    "ostream": "tapa::ostream<{type}>& {name}",
    "istreams": "tapa::istream<{type}>& {name}",
    "ostreams": "tapa::ostream<{type}>& {name}",
    "scalar": _SCALAR_PORT_TEMPLATE,
    "mmap": _MMAP_PORT_TEMPLATE,
    "hmap": _SCALAR_PORT_TEMPLATE,
    "async_mmap": _MMAP_PORT_TEMPLATE,
}


def gen_slot_cpp(slot_name: str, top_name: str, ports: list, top_cpp: str) -> str:
    """Generate floorplan slot cpp for hls synth.

    slot_name: Name of the slot
    ports: List of ports in the slot. Each port should match port format
        in tapa graph dict.
        e.g.
        {
            "cat": "istream",
            "name": "a",
            "type": "float",
            "width": 32
        }
    """
    cpp_ports = []
    cpp_pragmas = []
    for port in ports:
        assert isinstance(port, dict)
        assert "cat" in port
        assert "name" in port
        assert "type" in port
        assert "width" in port
        assert port["cat"] in _PORT_TEMPLATE
        port_type = port["type"]
        port_cat = port["cat"]

        # if "name[idx]" exists, replace it with "name_idx"
        match = re.fullmatch(r"([a-zA-Z_]\w*)\[(\d+)\]", port["name"])
        if match:
            n, i = match.groups()
            name = f"{n}_{i}"
        elif "[" in port and "]" in port:
            msg = f"Invalid port index in '{port}': must be a numeric index."
            raise ValueError(msg)
        else:
            name = port["name"]

        # when port is an array, find array element type
        # TODO: fix scalar cat due to mmap/streams
        if port_cat == "scalar":
            match = re.search(r"(?:tapa::)?(\w+)<([^,>]+)", port_type)
            if match:
                port_cat = match.group(1)
                port_type = match.group(2)

        # convert pointer to uint64_t
        if "*" in port_type:
            port_type = "uint64_t"

        # remove const from type for reinterpret_cast
        port_type = port_type.removeprefix("const ")

        cpp_ports.append(
            _PORT_TEMPLATE[port_cat].format(
                name=name,
                type=port_type,
            )
        )
        assert port_cat in _PRAGMA, port_cat
        cpp_pragmas.append(_PRAGMA[port_cat].format(name=name, type=port_type))
        continue

    pragma_body = "\n".join(cpp_pragmas)

    new_def = _env.get_template("slot_def.j2").render(
        name=slot_name,
        ports=", ".join(cpp_ports),
        pragma=pragma_body,
    )
    new_decl = _env.get_template("slot_decl.j2").render(
        name=slot_name,
        ports=", ".join(cpp_ports),
    )

    return replace_function(
        top_cpp,
        top_name,
        new_decl,
        new_def,
    )


_QUERY_FUNC = Query(
    _CPP_LANGUAGE,
    """
    (function_definition
      declarator: (function_declarator
        declarator: (identifier) @name)
      body: (compound_statement) @body)
    """,
)

_QUERY_EXTERN_FUNC = Query(
    _CPP_LANGUAGE,
    """
    (linkage_specification
      body: (function_definition
        declarator: (function_declarator
          declarator: (identifier) @name)
        body: (compound_statement) @body))
    """,
)


def _find_function_body(source: bytes, func_name: str) -> Node | None:
    """Find the compound_statement node for func_name using tree-sitter."""
    tree = _parser.parse(source)
    # Try plain function_definition first
    for _pattern_idx, captures in QueryCursor(_QUERY_FUNC).matches(tree.root_node):
        name_nodes = captures.get("name", [])
        body_nodes = captures.get("body", [])
        if name_nodes and name_nodes[0].text == func_name.encode():
            return body_nodes[0]
    # Try linkage_specification (extern "C") wrapping
    for _pattern_idx, captures in QueryCursor(_QUERY_EXTERN_FUNC).matches(
        tree.root_node
    ):
        name_nodes = captures.get("name", [])
        body_nodes = captures.get("body", [])
        if name_nodes and name_nodes[0].text == func_name.encode():
            return body_nodes[0]
    return None


def replace_function(
    source: str,
    func_name: str,
    new_body_or_decl: str,
    new_def: str | None = None,
) -> str:
    """Replace the body of func_name in source using tree-sitter CST.

    When called with two positional arguments (source, func_name, new_body),
    replaces only the compound_statement body of the named function.

    When called with four arguments (source, func_name, new_decl, new_def),
    uses the legacy remove-and-append strategy to replace both the extern "C"
    declaration and definition blocks.
    """
    if new_def is not None:
        # Legacy 4-arg path: remove old extern "C" blocks, append new ones.
        new_decl = new_body_or_decl
        code = _remove_extern_c_function_block(source, func_name, is_definition=False)
        code = _remove_extern_c_function_block(code, func_name, is_definition=True)
        decl_block = f'extern "C" {{\n{new_decl.strip()}\n}}  // extern "C"\n'
        def_block = f'extern "C" {{\n{new_def.strip()}\n}}  // extern "C"\n'
        return code.rstrip() + "\n\n" + decl_block + "\n\n" + def_block + "\n"

    # 3-arg path: tree-sitter body replacement.
    new_body = new_body_or_decl
    source_bytes = source.encode()
    body_node = _find_function_body(source_bytes, func_name)
    if body_node is None:
        msg = f"Function {func_name!r} not found"
        raise ValueError(msg)
    return (
        source_bytes[: body_node.start_byte].decode()
        + "{\n    "
        + new_body
        + "\n}"
        + source_bytes[body_node.end_byte :].decode()
    )


def _find_extern_c_function_block(
    code: str, func_name: str, is_definition: bool
) -> tuple[int, int] | None:
    """Find start & end index of the extern "C" block of the given function."""
    if is_definition:
        signature = rf"void\s+{re.escape(func_name)}\s*\([^)]*\)\s*\{{"
    else:
        signature = rf"void\s+{re.escape(func_name)}\s*\([^)]*\)\s*;"

    pattern = (
        rf'extern\s+"C"\s*\{{\s*'
        rf"{signature}.*?"
        rf'\}}\s*//\s*extern\s+"C"'
    )

    match = re.search(pattern, code, flags=re.DOTALL)
    if match:
        return match.start(), match.end()
    return None


def _remove_extern_c_function_block(
    code: str, func_name: str, is_definition: bool
) -> str:
    """Remove the extern "C" block containing the specified function."""
    bounds = _find_extern_c_function_block(code, func_name, is_definition)
    if bounds:
        start, end = bounds
        return code[:start] + code[end:]
    return code
