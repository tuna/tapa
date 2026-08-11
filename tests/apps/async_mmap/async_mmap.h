// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

#include <cstdint>

#include <tapa.h>

void AsyncTop(tapa::mmap<float> mem, tapa::mmap<float> dst, uint64_t n);
