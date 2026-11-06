#ifndef TAPA_CODEGEN_AIE_H_
#define TAPA_CODEGEN_AIE_H_

#include "backend.h"

namespace tapa::cc {

// The AI Engine backend. An AI Engine task keeps its TAPA signature and its
// body as written: `tapa/aie/tapa.h` gives the stream and vector types their
// AI Engine meaning, and the generated ADF kernel wrapper builds the ports.
// The backend only gives a `[[tapa::frame(N)]]` port its frame type in
// the task's own variant, and lowers loop attributes to chess keywords.
class AieBackend final : public Backend {
 public:
  void RewriteSignature(const TaskModel&, bool, EditSink&) const override {}
  void RewriteTaskFunc(const TaskModel& task, bool is_top,
                       EditSink& edits) const override;
  // Never the flow-wide backend: the flow is always a PL one.
  void StripOtherTask(const clang::FunctionDecl*, EditSink&) const override {}
  void RewriteHelperFunc(const clang::FunctionDecl*, EditSink&) const override {
  }
  void LowerPipeline(int ii, const std::string& style, const clang::Stmt* body,
                     EditSink& edits) const override;
  // A function-level pipeline has no chess form.
  void LowerFuncPipeline(int, const std::string&, const clang::Stmt* body,
                         EditSink& edits) const override;
  void LowerUnroll(int factor, const clang::Stmt* body,
                   EditSink& edits) const override;
  void LowerStmtAttr(const clang::Attr& attr, const clang::Stmt* body,
                     EditSink& edits) const override;
  void LowerDeclAttr(const clang::Attr& attr, const clang::VarDecl& var,
                     const clang::DeclStmt& decl,
                     EditSink& edits) const override;
  void LowerParamAttr(const clang::Attr& attr, const clang::ParmVarDecl& param,
                      const clang::Stmt* body, EditSink& edits) const override;
};

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_AIE_H_
