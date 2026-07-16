"""Small file-producing rules used by tests and packaging."""

# Copyright (c) 2026 RapidStream Design Automation, Inc. and contributors.
# All rights reserved. The contributor(s) of this file has/have agreed to the
# RapidStream Contributor License Agreement.

def _renamed_executable_impl(ctx):
    output = ctx.actions.declare_file(ctx.attr.out)
    args = ctx.actions.args()
    args.add(ctx.file.src)
    args.add(output)
    ctx.actions.run_shell(
        inputs = [ctx.file.src],
        outputs = [output],
        arguments = [args],
        command = "cp \"$1\" \"$2\" && chmod +x \"$2\"",
        mnemonic = "RenameExecutable",
    )
    return [DefaultInfo(files = depset([output]))]

renamed_executable = rule(
    implementation = _renamed_executable_impl,
    attrs = {
        "src": attr.label(
            allow_single_file = True,
            mandatory = True,
        ),
        "out": attr.string(mandatory = True),
    },
)

def _facebook_txt_impl(ctx):
    output = ctx.actions.declare_file(ctx.attr.out)
    args = ctx.actions.args()
    args.add(ctx.file.src)
    args.add(output)
    ctx.actions.run_shell(
        inputs = [ctx.file.src],
        outputs = [output],
        arguments = [args],
        command = "gzip -cd \"$1\" | sed 's/^0 /4039 /' > \"$2\"",
        mnemonic = "ProcessFacebookTxt",
    )
    return [DefaultInfo(files = depset([output]))]

facebook_txt = rule(
    implementation = _facebook_txt_impl,
    attrs = {
        "src": attr.label(
            allow_single_file = True,
            mandatory = True,
        ),
        "out": attr.string(mandatory = True),
    },
)

def _mdbook_tar_impl(ctx):
    output = ctx.outputs.out
    args = ctx.actions.args()
    args.add(ctx.file.mdbook)
    args.add(ctx.file.mdbook_admonish)
    args.add(ctx.file.book_toml)
    args.add(output)
    ctx.actions.run_shell(
        inputs = ctx.files.srcs + [ctx.file.book_toml],
        outputs = [output],
        tools = [
            ctx.file.mdbook,
            ctx.file.mdbook_admonish,
        ],
        arguments = [args],
        command = """
set -euo pipefail
MDBOOK="$1"
MDBOOK_ADMONISH="$2"
BOOK_TOML="$3"
OUT="$4"

WORK="$(mktemp -d)"
WORK_OUT="$(mktemp -d)"
trap 'rm -rf "$WORK" "$WORK_OUT"' EXIT

BOOK_DIR="$(dirname "$(realpath "$BOOK_TOML")")"
cp -rL "$BOOK_DIR/." "$WORK/"

BIN_DIR="$WORK/.bin"
mkdir -p "$BIN_DIR"
cp "$MDBOOK" "$BIN_DIR/mdbook"
cp "$MDBOOK_ADMONISH" "$BIN_DIR/mdbook-admonish"
chmod +x "$BIN_DIR/mdbook" "$BIN_DIR/mdbook-admonish"

PATH="$BIN_DIR:/usr/bin:/bin" "$BIN_DIR/mdbook-admonish" install "$WORK"
PATH="$BIN_DIR:/usr/bin:/bin" "$BIN_DIR/mdbook" build "$WORK" --dest-dir "$WORK_OUT/book"
tar -czf "$OUT" -C "$WORK_OUT" book
""",
        mnemonic = "MdbookBuild",
    )
    return [DefaultInfo(files = depset([output]))]

mdbook_tar = rule(
    implementation = _mdbook_tar_impl,
    attrs = {
        "book_toml": attr.label(
            allow_single_file = True,
            mandatory = True,
        ),
        "mdbook": attr.label(
            allow_single_file = True,
            cfg = "exec",
            mandatory = True,
        ),
        "mdbook_admonish": attr.label(
            allow_single_file = True,
            cfg = "exec",
            mandatory = True,
        ),
        "out": attr.output(mandatory = True),
        "srcs": attr.label_list(allow_files = True),
    },
)
