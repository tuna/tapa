#include "program.h"

namespace tapa::cc {

// The program model is currently pure data; this translation unit exists so the
// header is type-checked as it lands, ahead of the frontend modules (ports,
// discover, invoke_parser) that populate a `Program`. Building orchestration
// (`Program BuildProgram(clang::ASTContext&, ...)`) will live here.
static_assert(sizeof(Program) > 0, "Program must be a complete type");

}  // namespace tapa::cc
