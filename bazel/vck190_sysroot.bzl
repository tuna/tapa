"""Expose the installed Vitis aarch64 sysroot to the cross toolchain."""

load("@vars//:vars.bzl", "XILINX_TOOL_PATH", "XILINX_TOOL_VERSION")
load(":xilinx_versions.bzl", "xilinx_tool_dir_candidates")

# The GCC triple and version MODULE.bazel's cross toolchain looks under: its
# `multiarch` and `stdc++-<version>`, in toolchains_llvm's "yocto" layout.
# Whatever sysroot is found is presented under these names.
_TRIPLE = "aarch64-amd-linux"
_GCC_VERSION = "13.4.0"

# The sysroot's name in AMD's installs since 2025.1, then in Xilinx's.
_SYSROOT_NAMES = ["cortexa72-cortexa53-amd-linux", "aarch64-xilinx-linux"]

def _toolchain_links(root):
    """Map the toolchain's GCC paths to the sysroot's own, or None if it has none.

    The sysroot's libstdc++ is the one version under `usr/include/c++`, and
    its triple the one `aarch64-*` directory within that.
    """
    cxx = root.get_child("usr/include/c++")
    versions = cxx.readdir() if cxx.exists else []
    triples = [e for v in versions for e in v.readdir() if e.basename.startswith("aarch64-")]
    if len(versions) != 1 or len(triples) != 1:
        return None
    version, triple = versions[0], triples[0].basename
    headers = "usr/include/c++/{}/".format(_GCC_VERSION)
    links = {headers + e.basename: e for e in version.readdir() if e.basename != triple}
    links[headers + _TRIPLE] = version.get_child(triple)
    links["usr/lib/{}/{}".format(_TRIPLE, _GCC_VERSION)] = root.get_child("usr/lib", triple, version.basename)
    return links

def _link_sysroot(rctx, root, links):
    """Symlink `root`'s entries into `sysroot/`, with `links` laid over them.

    Each directory above a link is recreated, its entries symlinked one by
    one, so the link can sit among them.
    """
    above = {"": True}
    for link in links:
        parts = link.split("/")
        for i in range(1, len(parts)):
            above["/".join(parts[:i])] = True
    for parent in above:
        source = root.get_child(parent) if parent else root
        for entry in (source.readdir() if source.exists else []):
            path = parent + "/" + entry.basename if parent else entry.basename
            if path not in above and path not in links:
                rctx.symlink(entry, "sysroot/" + path)
    for path, target in links.items():
        rctx.symlink(target, "sysroot/" + path)

def _vck190_sysroot_impl(rctx):
    candidates = [
        "{}/gnu/aarch64/lin/aarch64-linux/{}".format(vitis, name)
        for vitis in xilinx_tool_dir_candidates(XILINX_TOOL_PATH, "Vitis", XILINX_TOOL_VERSION)
        for name in _SYSROOT_NAMES
    ]
    roots = [rctx.path(c) for c in candidates if rctx.path(c).exists]
    links = _toolchain_links(roots[0]) if roots else None
    if links == None:
        rctx.file(
            "sysroot/MISSING",
            "No Vitis aarch64 sysroot with libstdc++ among: {}\n".format(", ".join(candidates)),
        )
    else:
        _link_sysroot(rctx, roots[0], links)
    rctx.file(
        "sysroot/BUILD.bazel",
        """filegroup(
    name = "sysroot",
    srcs = ["."],
    visibility = ["//visibility:public"],
)
""",
    )

vck190_sysroot = repository_rule(
    implementation = _vck190_sysroot_impl,
    local = True,
)
