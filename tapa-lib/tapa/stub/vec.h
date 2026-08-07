// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

// `tapa/base/vec.h` uses CHECK_GE/CHECK_LE, so the target's logging must come
// first; `widthof` comes from util.h.
#include "tapa/stub/logging.h"
#include "tapa/stub/util.h"

#include "tapa/base/vec.h"
