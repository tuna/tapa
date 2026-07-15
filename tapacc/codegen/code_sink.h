#ifndef TAPA_CODEGEN_CODE_SINK_H_
#define TAPA_CODEGEN_CODE_SINK_H_

#include <initializer_list>
#include <string>
#include <vector>

#include "llvm/ADT/StringExtras.h"
#include "llvm/ADT/StringRef.h"

namespace tapa::cc {

// Accumulates generated lines and pragmas for a task body preamble, replacing
// the old `add_line` / `add_pragma` `std::function` pair with a concrete type.
class CodeSink {
 public:
  void Line(llvm::StringRef line) { lines_.push_back(line.str()); }

  void Pragma(std::initializer_list<llvm::StringRef> parts) {
    lines_.push_back("#pragma " + llvm::join(parts, " "));
  }

  bool Empty() const { return lines_.empty(); }
  const std::vector<std::string>& Lines() const { return lines_; }
  std::string Str() const { return llvm::join(lines_, "\n"); }

 private:
  std::vector<std::string> lines_;
};

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_CODE_SINK_H_
