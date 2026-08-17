"""Shared constants for tapacc BUILD files."""

# LLVM is built without RTTI; every tapacc TU must match.
TAPACC_COPTS = ["-fno-rtti"]

CLANG_TOOLING = "@tapa-llvm-project//clang:tooling"
