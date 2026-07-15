#include "backend.h"

namespace tapa::cc {

// Type-checks backend.h (and its cross-package frontend includes) as it lands,
// ahead of the concrete backends. The interface is otherwise header-only.
static_assert(sizeof(PortContext) > 0, "PortContext must be a complete type");

}  // namespace tapa::cc
