#pragma once

// `tapa/base/axis.h` builds its signals out of tapa::u<W>, so the target's
// integer layer must come first; `widthof` comes from util.h.
#include "tapa/base/int.h"
#include "tapa/stub/util.h"

#include "tapa/base/axis.h"
