#ifndef TAPA_FRONTEND_PORTS_H_
#define TAPA_FRONTEND_PORTS_H_

#include <vector>

#include "clang/AST/ASTContext.h"
#include "clang/AST/Decl.h"

#include "program.h"

namespace tapa::cc {

// Extract the external ports of a task from its parameters: an `mmaps<T, N>`
// expands to N per-channel `mmap` ports (`name[i]`); `istreams`/`ostreams`
// and `hmap` stay single but carry `chan_count` (and `hmap` a `chan_size`);
// everything else is one port.
std::vector<Port> BuildPorts(const clang::ASTContext& ctx,
                             const clang::FunctionDecl* task);

// Reads `[[tapa::frame(N)]]` onto a leaf task's stream ports on every target,
// from every redeclaration so a forward declaration and its definition agree.
// Reports an error for a frame on another kind of task or port, or with a
// non-positive N, and for a frame port (`tapa::iframes`/`oframes`, whose type
// sets its frame) on an upper task.
void ReadFrameAttrs(const clang::ASTContext& ctx, TaskModel& task);

}  // namespace tapa::cc

#endif  // TAPA_FRONTEND_PORTS_H_
