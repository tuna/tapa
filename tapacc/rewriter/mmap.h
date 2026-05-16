// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#ifndef TAPA_MMAP_H_
#define TAPA_MMAP_H_

#include <string>

#include "clang/AST/AST.h"

#include "type.h"

// Works for mmap, mmaps, async_mmap, async_mmaps, hmap, immap, and ommap.
std::string GetMmapElemType(const clang::ParmVarDecl* param);

#endif  // TAPA_MMAP_H_
