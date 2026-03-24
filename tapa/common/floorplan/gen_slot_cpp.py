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
        assert {"cat", "name", "type", "width"} <= port.keys()
        assert port["cat"] in _PORT_TEMPLATE
        port_type = port["type"]
        port_cat = port["cat"]

        match = re.fullmatch(r"([a-zA-Z_]\w*)\[(\d+)\]", port["name"])
        if match:
            n, i = match.groups()
            name = f"{n}_{i}"
        elif "[" in port["name"]:
            msg = f"Invalid port index in '{port['name']}': must be a numeric index."
            raise ValueError(msg)
        else:
            name = port["name"]

        # TODO: fix scalar cat due to mmap/streams
        if port_cat == "scalar":
            m = re.search(r"(?:tapa::)?(\w+)<([^,>]+)", port_type)
            if m:
                port_cat = m.group(1)
                port_type = m.group(2)

        if "*" in port_type:
            port_type = "uint64_t"

        port_type = port_type.removeprefix("const ")

        cpp_ports.append(
            _PORT_TEMPLATE[port_cat].format(
                name=name,
                type=port_type,
            )
        )
        assert port_cat in _PRAGMA, port_cat
        cpp_pragmas.append(_PRAGMA[port_cat].format(name=name, type=port_type))

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

# Captures braced extern "C" { void foo() {} } linkage_specification nodes
# (body is declaration_list containing function_definition).
_QUERY_EXTERN_LINKAGE_BRACED_DEF = Query(
    _CPP_LANGUAGE,
    """
    (linkage_specification
      (declaration_list
        (function_definition
          declarator: (function_declarator
            declarator: (identifier) @name)))) @linkage
    """,
)

# Captures braced extern "C" { void foo(); } linkage_specification nodes
# (body is declaration_list containing declaration).
_QUERY_EXTERN_LINKAGE_BRACED_DECL = Query(
    _CPP_LANGUAGE,
    """
    (linkage_specification
      (declaration_list
        (declaration
          declarator: (function_declarator
            declarator: (identifier) @name)))) @linkage
    """,
)

# Captures inline extern "C" void foo() {} linkage_specification nodes
# (body is a direct function_definition, no declaration_list).
_QUERY_EXTERN_LINKAGE_INLINE = Query(
    _CPP_LANGUAGE,
    """
    (linkage_specification
      body: (function_definition
        declarator: (function_declarator
          declarator: (identifier) @name))) @linkage
    """,
)


def _find_extern_c_linkage_nodes(source: bytes, func_name: str) -> list[Node]:
    """Return all linkage_specification nodes wrapping func_name (decl or def).

    Handles both inline form (extern "C" void foo() {}) and braced form
    (extern "C" { void foo() {} } or extern "C" { void foo(); }).
    """
    tree = _parser.parse(source)
    nodes: list[Node] = []
    func_bytes = func_name.encode()
    queries = (
        _QUERY_EXTERN_LINKAGE_BRACED_DEF,
        _QUERY_EXTERN_LINKAGE_BRACED_DECL,
        _QUERY_EXTERN_LINKAGE_INLINE,
    )
    for query in queries:
        for _pattern_idx, captures in QueryCursor(query).matches(tree.root_node):
            name_nodes = captures.get("name", [])
            linkage_nodes = captures.get("linkage", [])
            if name_nodes and name_nodes[0].text == func_bytes and linkage_nodes:
                nodes.append(linkage_nodes[0])
    return nodes


def _find_function_body(source: bytes, func_name: str) -> Node | None:
    """Find the compound_statement node for func_name using tree-sitter."""
    tree = _parser.parse(source)
    func_bytes = func_name.encode()
    for query in (_QUERY_FUNC, _QUERY_EXTERN_FUNC):
        for _pattern_idx, captures in QueryCursor(query).matches(tree.root_node):
            name_nodes = captures.get("name", [])
            body_nodes = captures.get("body", [])
            if name_nodes and name_nodes[0].text == func_bytes:
                return body_nodes[0]
    return None


def replace_function(
    source: str,
    func_name: str,
    new_body_or_decl: str,
    new_def: str | None = None,
) -> str:
    """Replace the body of func_name in source using tree-sitter CST.

    When called with three positional arguments (source, func_name, new_body),
    replaces only the compound_statement body of the named function.

    When called with four arguments (source, func_name, new_decl, new_def),
    removes all extern "C" linkage_specification nodes for the function using
    tree-sitter byte-offset splicing, then appends the new declaration and
    definition blocks.
    """
    if new_def is not None:
        new_decl = new_body_or_decl
        source_bytes = source.encode()
        linkage_nodes = _find_extern_c_linkage_nodes(source_bytes, func_name)
        linkage_nodes.sort(key=lambda n: n.start_byte, reverse=True)
        for node in linkage_nodes:
            end = node.end_byte
            rest = source_bytes[end:]
            comment_match = re.match(rb'\s*//\s*extern\s+"C"', rest)
            if comment_match:
                end += comment_match.end()
            source_bytes = source_bytes[: node.start_byte] + source_bytes[end:]
        code = source_bytes.decode()
        decl_block = f'extern "C" {{\n{new_decl.strip()}\n}}  // extern "C"\n'
        def_block = f'extern "C" {{\n{new_def.strip()}\n}}  // extern "C"\n'
        return code.rstrip() + "\n\n" + decl_block + "\n\n" + def_block + "\n"

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
