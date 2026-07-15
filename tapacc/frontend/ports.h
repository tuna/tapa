#ifndef TAPA_FRONTEND_PORTS_H_
#define TAPA_FRONTEND_PORTS_H_

#include <vector>

#include "clang/AST/ASTContext.h"
#include "clang/AST/Decl.h"

#include "program.h"

namespace tapa::cc {

// Extract the external ports of a task from its parameters, matching the old
// ProcessTaskPorts: an `mmaps<T, N>` expands to N per-channel `mmap` ports
// (`name[i]`); `istreams`/`ostreams` and `hmap` stay single but carry
// `chan_count` (and `hmap` a `chan_size`); everything else is one port.
std::vector<Port> BuildPorts(const clang::ASTContext& ctx,
                             const clang::FunctionDecl* task);

}  // namespace tapa::cc

#endif  // TAPA_FRONTEND_PORTS_H_
