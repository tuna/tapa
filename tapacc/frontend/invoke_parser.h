#ifndef TAPA_FRONTEND_INVOKE_PARSER_H_
#define TAPA_FRONTEND_INVOKE_PARSER_H_

#include <string>

#include "clang/AST/ASTContext.h"
#include "llvm/ADT/STLFunctionalExtras.h"

#include "program.h"

namespace tapa::cc {

// Parse an upper task's `tapa::stream` declarations and `tapa::task::invoke`
// calls into `task.streams` (depth + producer/consumer endpoints) and
// `task.instances` (per child-task-name instantiation lists with resolved
// arguments). Handles vectorized invokes, array (streams/mmaps) element
// mapping, `seq`, `executable`, integer literals, and explicit instance names.
// When `is_top` is set, stream-typed arguments that name the task's own ports
// (rather than a local FIFO) are recorded as depth-less external FIFOs with a
// single endpoint, so the kernel-boundary streams appear in the task graph.
// Reports stream wiring errors (double produce/consume, half-connected) and
// prunes unused streams via `ctx` diagnostics. A no-op for a leaf task.
// `parameter_name` resolves a callee's original definition formal by position;
// caller-visible declarations may rename or omit it. The callback stays
// TU-local.
void ParseUpperTask(
    clang::ASTContext& ctx, TaskModel& task, bool is_top,
    llvm::function_ref<std::string(const clang::FunctionDecl*, unsigned)>
        parameter_name);

// Report every `tapa::task::invoke` in a task's body that the task graph
// would not hold: one outside the first `tapa::task()` statement written
// directly in the body (a second such statement, a nested block, a loop, a
// declaration's initializer), which in a leaf means any invoke at all.
// Hardware keeps only that first statement's invokes and HLS runs any other
// as a sequential call, so each would silently change the program.
void DiagnoseStrayInvokes(clang::ASTContext& ctx,
                          const clang::FunctionDecl* func);

}  // namespace tapa::cc

#endif  // TAPA_FRONTEND_INVOKE_PARSER_H_
