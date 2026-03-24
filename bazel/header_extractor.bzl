"""Custom rule to extract headers from dependencies."""

# Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
# All rights reserved. The contributor(s) of this file has/have agreed to the
# RapidStream Contributor License Agreement.

load("@rules_cc//cc/common:cc_info.bzl", "CcInfo")

def _header_extractor_impl(ctx):
    headers = depset(transitive = [
        dep[CcInfo].compilation_context.headers
        for dep in ctx.attr.deps
        if CcInfo in dep
    ])

    headers_list = [h for h in headers.to_list() if "_virtual_includes" in h.path]

    for dep in ctx.attr.deps:
        if CcInfo not in dep:
            headers_list.extend(dep.files.to_list())

    output_files = []
    for header in depset(headers_list).to_list():
        if "_virtual_includes/" in header.path:
            header_name = header.path.split("_virtual_includes/")[-1]
            header_name = header_name.split("/", 1)[-1]
        elif "include/" in header.path:
            header_name = header.path.split("include/")[-1]
        else:
            header_name = header.path

        output_file = ctx.actions.declare_file(ctx.label.name + "/" + header_name)
        ctx.actions.run_shell(
            outputs = [output_file],
            inputs = [header],
            command = "cp '{}' '{}'".format(header.path, output_file.path),
        )
        output_files.append(output_file)

    return [DefaultInfo(files = depset(output_files))]

header_extractor = rule(
    implementation = _header_extractor_impl,
    attrs = {
        "deps": attr.label_list(
            default = [],
        ),
    },
)
