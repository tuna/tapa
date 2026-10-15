#!/usr/bin/env python3
"""Export a TAPA XO's IP and argument ABI for SLASH without changing RTL."""

import argparse
import copy
import io
import json
import os
import re
import tarfile
import tempfile
from pathlib import Path
from xml.etree import ElementTree as ET
from zipfile import ZipFile

SPIRIT = "http://www.spiritconsortium.org/XMLSchema/SPIRIT/1685-2009"
NS = {"s": SPIRIT}
ET.register_namespace("spirit", SPIRIT)
ET.register_namespace("xilinx", "http://www.xilinx.com")
V80_KERNEL_BASE = 0x020200010000


def add(parent, tag, value=None):
    child = ET.SubElement(parent, f"{{{SPIRIT}}}{tag}")
    if value is not None:
        child.text = str(value)
    return child


def register(block, name, offset):
    reg = add(block, "register")
    for tag, value in [
        ("name", name),
        ("addressOffset", hex(offset)),
        ("size", 32),
        ("access", "read-write"),
    ]:
        add(reg, tag, value)


def argument_metadata(root, kernel):
    """Translate kernel.xml's ABI into IP-XACT registers and SLASH hwRefs."""
    block = root.find(
        's:memoryMaps/s:memoryMap[s:name="s_axi_control"]/s:addressBlock', NS
    )
    if block is None or block.findtext("s:usage", namespaces=NS) != "register":
        raise ValueError("Missing control register address block")
    if block.findall("s:register", NS):
        raise ValueError("Existing register map requires review")
    interfaces = {
        b.findtext("s:name", namespaces=NS): b
        for b in root.findall("s:busInterfaces/s:busInterface", NS)
    }
    args = {}
    occupied = set(range(16))
    for name, offset in [("CTRL", 0), ("GIER", 4), ("IP_IER", 8), ("IP_ISR", 12)]:
        register(block, name, offset)
    for index, arg in enumerate(kernel.findall("args/arg")):
        name = arg.get("name")
        qualifier = int(arg.get("addressQualifier"))
        size = int(arg.get("size"), 0)
        offset = int(arg.get("offset"), 0)
        port = arg.get("port")
        if (
            not name
            or name in args
            or int(arg.get("id")) != index
            or qualifier not in (0, 1)
            or port not in interfaces
            or (qualifier == 0 and port != "s_axi_control")
            or (qualifier == 1 and interfaces[port].find("s:master", NS) is None)
            or size not in (4, 8)
            or (qualifier == 1 and size != 8)
            or offset % 4
            or offset + size > int(block.findtext("s:range", namespaces=NS), 0)
            or occupied.intersection(range(offset, offset + size))
        ):
            raise ValueError(f"Unsupported or inconsistent argument: {name}")
        occupied.update(range(offset, offset + size))
        refs = []
        for word in range(size // 4):
            reg_name = f"{name}_{word}"
            register(block, reg_name, offset + word * 4)
            refs.append(
                {
                    "type": "register",
                    "interface": "s_axi_control",
                    "name": reg_name,
                    "usage": "address" if qualifier else "data",
                }
            )
        if qualifier:
            refs.append({"type": "interface", "interface": port})
        args[name] = {
            "index": index,
            "srcType": arg.get("type"),
            "srcSize": int(arg.get("hostSize"), 0) * 8,
            "hwRefs": refs,
        }
    return {"Args": args}


def export(xo, output):
    if output.exists():
        raise ValueError("Output must not already exist")
    with ZipFile(xo) as archive:
        components = [n for n in archive.namelist() if n.endswith("/component.xml")]
        kernels = [n for n in archive.namelist() if n.endswith("/kernel.xml")]
        if len(components) != 1 or len(kernels) != 1:
            raise ValueError("Expected exactly one packaged kernel")
        kernel_bytes = archive.read(kernels[0])
        kernel = ET.fromstring(kernel_bytes).find("kernel")
        if kernel is None or kernel.get("hwControlProtocol") != "ap_ctrl_hs":
            raise ValueError("Only ap_ctrl_hs kernels are supported")
        component_name = components[0]
        prefix = str(Path(component_name).parent) + "/"
        root = ET.fromstring(archive.read(component_name))
        vlnv = ":".join(
            root.findtext(f"s:{tag}", namespaces=NS)
            for tag in ("vendor", "library", "name", "version")
        )
        if vlnv != kernel.get("vlnv"):
            raise ValueError("IP component and kernel.xml VLNV differ")
        metadata = argument_metadata(root, kernel)
        ip = output / "impl" / "ip"
        entries = []
        for info in archive.infolist():
            if info.filename.startswith(prefix) and not info.is_dir():
                relative = Path(info.filename[len(prefix):])
                if relative.is_absolute() or ".." in relative.parts:
                    raise ValueError("Unsafe IP archive path")
                entries.append((info, ip / relative))
        for info, target in entries:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(archive.read(info))
        ET.indent(root)
        ET.ElementTree(root).write(
            ip / "component.xml", encoding="UTF-8", xml_declaration=True
        )
        (output / "hls_data.json").write_text(json.dumps(metadata, indent=4) + "\n")
        (output / "kernel.xml").write_bytes(kernel_bytes)
        return ip / "component.xml"


def _updated_system_map(data):
    root = ET.fromstring(data)
    if root.findtext("Platform") != "Hardware" or root.findtext("ShellType") != "service":
        raise ValueError("Expected a V80 hardware service-shell system map")
    kernels = root.findall("Kernel")
    if len(kernels) != 1:
        raise ValueError("Expected exactly one V80 service-shell kernel")
    base = kernels[0].find("BaseAddress")
    if base is None:
        raise ValueError("V80 kernel has no control base address")
    current = int(base.text, 0)
    if current not in (0x020200000000, V80_KERNEL_BASE):
        raise ValueError(f"Unexpected V80 kernel control base {current:#x}")
    base.text = f"0x{V80_KERNEL_BASE:X}"
    ET.indent(root, space="  ")
    return ET.tostring(root, encoding="utf-8", xml_declaration=True)


def _rewrite_vbin_system_map(vbin, system_map):
    fd, temporary = tempfile.mkstemp(prefix=f".{vbin.name}.", dir=vbin.parent)
    os.close(fd)
    found = 0
    try:
        with tarfile.open(vbin, "r:gz") as source, tarfile.open(temporary, "w:gz") as target:
            for member in source:
                if member.name == "system_map.xml":
                    found += 1
                    info = copy.copy(member)
                    info.size = len(system_map)
                    target.addfile(info, io.BytesIO(system_map))
                elif member.isfile():
                    stream = source.extractfile(member)
                    if stream is None:
                        raise ValueError(f"Cannot read VBIN member {member.name}")
                    target.addfile(member, stream)
                else:
                    target.addfile(member)
        if found != 1:
            raise ValueError(f"Expected one VBIN system_map.xml, found {found}")
        os.replace(temporary, vbin)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def finalize(vbin):
    """Synchronize VRT metadata with the address assigned in the Vivado BD."""
    project_map = Path(f"{vbin}.prj") / "system_map.xml"
    if not project_map.is_file():
        raise ValueError(f"SLASH project has no system map: {project_map}")
    system_map = _updated_system_map(project_map.read_bytes())
    _rewrite_vbin_system_map(vbin, system_map)
    project_map.write_bytes(system_map)
    print(f"Updated {vbin} kernel control base to {V80_KERNEL_BASE:#x}")


def verify(vbin, clock_hz):
    """Require the requested clock, safe service-shell ABI and routed timing."""
    with tarfile.open(vbin, "r:gz") as archive:
        stream = archive.extractfile("system_map.xml")
        if stream is None:
            raise ValueError("VBIN has no system_map.xml")
        root = ET.parse(stream).getroot()
        if root.findtext("Platform") != "Hardware":
            raise ValueError("VBIN is not a hardware image")
        if root.findtext("ShellType") != "service":
            raise ValueError("Expected the V80 service shell")
        if int(root.findtext("ClockFrequency")) != clock_hz:
            raise ValueError("SLASH changed the requested clock; timing did not close")
        kernels = root.findall("Kernel")
        if len(kernels) != 1 or int(kernels[0].findtext("BaseAddress"), 0) != V80_KERNEL_BASE:
            raise ValueError("Expected one kernel after the service-shell 64 KiB reservation")
        if not any(m.name.endswith(".pdi") and m.size for m in archive.getmembers()):
            raise ValueError("VBIN has no programming image")
    reports = list(Path(str(vbin) + ".prj").glob("slash_rm/report_timing_*.txt"))
    if len(reports) != 1:
        raise ValueError("Expected one final routed timing report")
    report = reports[0].read_text()
    match = re.search(
        r"WNS\(ns\)\s+TNS\(ns\).*?\n\s*-+[^\n]*\n\s*"
        r"([-+\d.]+)\s+([-+\d.]+)\s+\d+\s+\d+\s+([-+\d.]+)\s+([-+\d.]+)",
        report, re.DOTALL,
    )
    if match is None or any(float(value) < 0 for value in match.groups()):
        raise ValueError("Final routed setup/hold timing is missing or failing")
    if "Timing constraints are not met" in report:
        raise ValueError("Final timing constraints are not met")
    print(f"Verified {vbin} at {clock_hz} Hz; timing: {reports[0]}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    exporter = commands.add_parser("export")
    exporter.add_argument("xo", type=Path)
    exporter.add_argument("output", type=Path)
    finalizer = commands.add_parser("finalize")
    finalizer.add_argument("vbin", type=Path)
    checker = commands.add_parser("verify")
    checker.add_argument("vbin", type=Path)
    checker.add_argument("clock_hz", type=int)
    args = parser.parse_args()
    try:
        if args.command == "export":
            print(export(args.xo, args.output))
        elif args.command == "finalize":
            finalize(args.vbin)
        else:
            verify(args.vbin, args.clock_hz)
    except (ValueError, OSError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
