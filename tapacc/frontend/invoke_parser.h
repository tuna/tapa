#ifndef TAPA_FRONTEND_INVOKE_PARSER_H_
#define TAPA_FRONTEND_INVOKE_PARSER_H_

#include "clang/AST/ASTContext.h"

#include "program.h"

namespace tapa::cc {

// Parse an upper task's `tapa::stream` declarations and `tapa::task::invoke`
// calls into `task.streams` (depth + producer/consumer endpoints) and
// `task.instances` (per child-task-name instantiation lists with resolved
// arguments). Handles vectorized invokes, array (streams/mmaps) element
// mapping, `seq`, `executable`, integer literals, and explicit instance names.
// Reports stream wiring errors (double produce/consume, half-connected) and
// prunes unused streams via `ctx` diagnostics. A no-op for a leaf task.
void ParseUpperTask(clang::ASTContext& ctx, TaskModel& task);

}  // namespace tapa::cc

#endif  // TAPA_FRONTEND_INVOKE_PARSER_H_
