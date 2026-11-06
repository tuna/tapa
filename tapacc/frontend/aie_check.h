#ifndef TAPA_FRONTEND_AIE_CHECK_H_
#define TAPA_FRONTEND_AIE_CHECK_H_

#include <cstdint>
#include <set>
#include <string>

#include "clang/AST/ASTContext.h"
#include "clang/AST/Decl.h"

#include "program.h"

namespace tapa::cc {

// The AI Engine eligibility rules for one AI Engine task: memory only
// through async_mmap, stream and memory elements a multiple of 32 bits, and
// (for a leaf) only the blocking stream operations -- AI Engine streams
// carry no end-of-transmission token and expose no occupancy. Each violation
// is an error at its source site.
void CheckAieTask(const clang::ASTContext& ctx, const TaskModel& task);

// The directions a body uses of its async_mmap parameter: reads when it
// names `read_addr` or `read_data`, writes when it names `write_addr`,
// `write_data` or `write_resp`, and responses when it names `write_resp`;
// all of them when it hands the parameter on whole (to a helper, say),
// since the members are then out of sight.
struct MemoryUse {
  bool reads = false;
  bool writes = false;
  bool responses = false;
};
MemoryUse UsedDirections(const clang::FunctionDecl* def,
                         const clang::ParmVarDecl* param);

// The bytes of a leaf body's local variables (every non-static local of a
// known size, whatever its scope): an AI Engine kernel keeps them on its
// stack, which the graph sizes from this.
uint32_t LocalBytes(const clang::ASTContext& ctx,
                    const clang::FunctionDecl* def);

// The stream ports a leaf body closes (`close()` / `try_close()`).
std::set<std::string> ClosedPorts(const clang::FunctionDecl* def);

}  // namespace tapa::cc

#endif  // TAPA_FRONTEND_AIE_CHECK_H_
