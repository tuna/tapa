"""Shared constants for tapacc BUILD files."""

# Copyright (c) 2026 RapidStream Design Automation, Inc. and contributors.
# All rights reserved. The contributor(s) of this file has/have agreed to the
# RapidStream Contributor License Agreement.

# LLVM is built without RTTI; every tapacc TU must match.
TAPACC_COPTS = ["-fno-rtti"]

CLANG_TOOLING = "@tapa-llvm-project//clang:tooling"
