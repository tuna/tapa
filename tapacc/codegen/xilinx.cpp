#include "xilinx.h"

#include <string>
#include <vector>

#include "clang/AST/Decl.h"
#include "clang/AST/Stmt.h"
#include "clang/AST/TypeLoc.h"
#include "clang/Lex/Lexer.h"
#include "llvm/Support/raw_ostream.h"

#include "../frontend/classify.h"
#include "../frontend/type_args.h"
#include "conventions.h"
#include "emit.h"

namespace tapa::cc {

namespace {

// The effective code-generation level of a function. The top-level distinction
// only exists in Vitis mode; in plain Vitis HLS the top task is generated as a
// middle-level upper task (old RewriteTopLevelFunc -> RewriteMiddleLevelFunc).
enum class Lvl { kTop, kMiddle, kLower };

Lvl LvlOf(TaskLevel level, bool is_top, bool is_vitis) {
  if (level == TaskLevel::kLower) return Lvl::kLower;
  if (is_top && is_vitis) return Lvl::kTop;
  return Lvl::kMiddle;
}

bool IsMmapArray(TapaKind k) {
  return k == TapaKind::kMmaps || k == TapaKind::kHmap;
}

int64_t ArraySize(const clang::ParmVarDecl* param) {
  return IntTemplateArg(param->getType(), 1).value_or(0);
}

// AddCodeForLowerLevelStream: HLS interface pragmas for a stream port's
// FIFO(s).
void EmitLowerStream(const clang::ParmVarDecl* param, TapaKind kind,
                     CodeSink& out) {
  const std::string name = param->getNameAsString();
  out.Pragma({"HLS disaggregate variable =", name});

  std::vector<std::string> names;
  if (kind == TapaKind::kIStreams || kind == TapaKind::kOStreams) {
    out.Pragma({"HLS array_partition variable =", name, "complete"});
    for (int64_t i = 0; i < ArraySize(param); ++i) {
      names.push_back(ArrayNameAt(name, static_cast<int>(i)));
    }
  } else {
    names.push_back(name);
  }

  const bool is_input = IsInputStream(kind);
  for (const std::string& n : names) {
    const std::string fifo = FifoVar(n);
    out.Pragma({"HLS interface ap_fifo port =", fifo});
    out.Pragma({"HLS aggregate variable =", fifo, "bit"});
    if (is_input) {
      const std::string peek = PeekVar(n);
      out.Pragma({"HLS interface ap_fifo port =", peek});
      out.Pragma({"HLS aggregate variable =", peek, "bit"});
      out.Line("void(" + n + "._.empty());");
      out.Line("void(" + n + "._peek.empty());");
    } else {
      out.Line("void(" + n + "._.full());");
    }
  }
}

// AddCodeForLowerLevelAsyncMmap: the five FIFOs of an async_mmap.
void EmitLowerAsyncMmap(const clang::ParmVarDecl* param, CodeSink& out) {
  const std::string name = param->getNameAsString();
  out.Pragma({"HLS disaggregate variable =", name});
  for (const char* tag : {".read_addr", ".read_data", ".write_addr",
                          ".write_data", ".write_resp"}) {
    const std::string fifo = FifoVar(name + tag);
    out.Pragma({"HLS interface ap_fifo port =", fifo});
    out.Pragma({"HLS aggregate variable =", fifo, " bit"});
  }
  for (const char* tag : {".read_data", ".write_resp"}) {
    out.Pragma({"HLS disaggregate variable =", name, tag});
    const std::string peek = PeekVar(name + tag);
    out.Pragma({"HLS interface ap_fifo port =", peek});
    out.Pragma({"HLS aggregate variable =", peek, "bit"});
  }
  out.Line("void(" + name + ".read_addr._.full());");
  out.Line("void(" + name + ".read_data._.empty());");
  out.Line("void(" + name + ".read_data._peek.empty());");
  out.Line("void(" + name + ".write_addr._.full());");
  out.Line("void(" + name + ".write_data._.full());");
  out.Line("void(" + name + ".write_resp._.empty());");
  out.Line("void(" + name + ".write_resp._peek.empty());");
}

// Generate the body-preamble text for a task at the given level.
std::string GeneratePreamble(const Backend& backend, const TaskModel& task,
                             bool is_top) {
  CodeSink sink;
  for (const clang::ParmVarDecl* param : task.def->parameters()) {
    const PortContext p{param, ClassifyTapaType(param), task.level, is_top};
    backend.EmitPortPreamble(p, sink);
    sink.Line("");  // blank line between parameters
  }
  return sink.Str();
}

// Vitis HLS rejects `inline` on task functions; strip the leading keyword.
void RemoveInline(const clang::FunctionDecl* func, clang::Rewriter& rewriter) {
  if (!func->isInlineSpecified()) return;
  clang::Token token;
  clang::Lexer::getRawToken(func->getBeginLoc(), token, rewriter.getSourceMgr(),
                            rewriter.getLangOpts());
  if (token.getRawIdentifier().str() == "inline") {
    rewriter.RemoveText(token.getLocation(), token.getLength());
  } else {
    llvm::errs() << "Warning: expected 'inline' at the start of a task; not "
                    "removed. Vitis HLS does not support inline tasks.\n";
  }
}

// Add `#pragma HLS stream` depth pragmas after each stream declaration.
void RewriteStreamDefinitions(const clang::FunctionDecl* func,
                              clang::Rewriter& rewriter) {
  if (!func->hasBody()) return;
  for (const clang::Stmt* child : func->getBody()->children()) {
    const auto* decl_stmt = llvm::dyn_cast<clang::DeclStmt>(child);
    if (decl_stmt == nullptr || !decl_stmt->isSingleDecl()) continue;
    const auto* var =
        llvm::dyn_cast<clang::VarDecl>(decl_stmt->getSingleDecl());
    if (var == nullptr) continue;
    if (ClassifyTapaType(var->getType()) == TapaKind::kStream) {
      const int64_t depth = IntTemplateArg(var->getType(), 1).value_or(0);
      AddPragmaAfterStmt(rewriter, decl_stmt,
                         "HLS stream variable = " + var->getNameAsString() +
                             "._ depth = " + std::to_string(depth));
    }
  }
}

}  // namespace

void XilinxBackend::EmitStreamPort(const PortContext& p, CodeSink& out) const {
  const Lvl lvl = LvlOf(p.level, p.is_top, is_vitis_);
  if (lvl == Lvl::kTop && is_vitis_) {
    out.Pragma({"HLS interface axis port =", p.param->getNameAsString()});
    EmitDummyStreamRW(p.param, p.kind, out, /*qdma=*/true);
    return;
  }
  EmitLowerStream(p.param, p.kind, out);
  if (lvl != Lvl::kLower) {  // middle / top-non-vitis also need anti-DCE reads
    EmitDummyStreamRW(p.param, p.kind, out, /*qdma=*/false);
  }
}

void XilinxBackend::EmitMmapPort(const PortContext& p, CodeSink& out) const {
  const Lvl lvl = LvlOf(p.level, p.is_top, is_vitis_);
  const std::string name = p.param->getNameAsString();
  switch (lvl) {
    case Lvl::kLower:
      if (p.kind == TapaKind::kMmaps) {
        out.Line("#error mmaps not supported for lower level tasks");
      } else if (p.kind == TapaKind::kHmap) {
        out.Line("#error hmap not supported for lower level tasks");
      } else {
        out.Pragma({"HLS interface m_axi port =", name,
                    "offset = direct bundle =", name});
      }
      break;
    case Lvl::kMiddle:
      if (IsMmapArray(p.kind)) {
        for (int64_t i = 0; i < ArraySize(p.param); ++i) {
          out.Pragma({"HLS interface ap_none port =",
                      ArrayElemOffset(name, static_cast<int>(i)), "register"});
        }
      } else {
        out.Pragma(
            {"HLS interface ap_none port =", OffsetName(name), "register"});
      }
      EmitDummyMmapOrScalarRW(p.param, p.kind, out);
      break;
    case Lvl::kTop:
      if (!is_vitis_) {
        out.Line("#error top-level mmaps not supported in non-Vitis mode");
        return;
      }
      if (IsMmapArray(p.kind)) {
        for (int64_t i = 0; i < ArraySize(p.param); ++i) {
          out.Pragma({"HLS interface s_axilite port =",
                      ArrayElemOffset(name, static_cast<int>(i)),
                      "bundle = control"});
        }
      } else {
        out.Pragma({"HLS interface s_axilite port =", OffsetName(name),
                    "bundle = control"});
      }
      EmitDummyMmapOrScalarRW(p.param, p.kind, out);
      break;
  }
}

void XilinxBackend::EmitAsyncMmapPort(const PortContext& p,
                                      CodeSink& out) const {
  const Lvl lvl = LvlOf(p.level, p.is_top, is_vitis_);
  if (lvl == Lvl::kLower) {
    EmitLowerAsyncMmap(p.param, out);
  } else {
    // Middle/top async_mmap is handled like a scalar offset register (old
    // AddCodeForMiddleLevelAsyncMmap -> scalar; top -> mmap).
    EmitScalarPort(p, out);
  }
}

void XilinxBackend::EmitScalarPort(const PortContext& p, CodeSink& out) const {
  const Lvl lvl = LvlOf(p.level, p.is_top, is_vitis_);
  const std::string name = p.param->getNameAsString();
  switch (lvl) {
    case Lvl::kLower:
      break;  // no interface pragma for lower-level scalars
    case Lvl::kMiddle:
      out.Pragma({"HLS interface ap_none port =", name, "register"});
      EmitDummyMmapOrScalarRW(p.param, p.kind, out);
      break;
    case Lvl::kTop:
      if (is_vitis_) {
        out.Pragma(
            {"HLS interface s_axilite port =", name, "bundle = control"});
        EmitDummyMmapOrScalarRW(p.param, p.kind, out);
      } else {
        out.Pragma({"HLS interface ap_none port =", name, "register"});
        EmitDummyMmapOrScalarRW(p.param, p.kind, out);
      }
      break;
  }
}

void XilinxBackend::RewriteSignature(const TaskModel& task, bool is_top,
                                     clang::Rewriter& rewriter) const {
  const Lvl lvl = LvlOf(task.level, is_top, is_vitis_);
  if (lvl == Lvl::kLower) return;  // lower-level signatures are unchanged

  // Middle/top: replace mmap parameters with 64-bit base addresses, on every
  // declaration of the function (the forward declaration and the definition),
  // so the emitted signatures agree.
  for (const clang::FunctionDecl* decl : task.def->redecls()) {
    for (const clang::ParmVarDecl* param : decl->parameters()) {
      const std::string name = param->getNameAsString();
      const TapaKind kind = ClassifyTapaType(param);
      if (kind == TapaKind::kMmap || kind == TapaKind::kAsyncMmap) {
        rewriter.ReplaceText(
            param->getTypeSourceInfo()->getTypeLoc().getSourceRange(),
            "uint64_t");
        rewriter.ReplaceText(param->getLocation(), OffsetName(name));
      } else if (IsMmapArray(kind)) {
        std::string text;
        for (int64_t i = 0; i < ArraySize(param); ++i) {
          if (!text.empty()) text += ", ";
          text += "uint64_t " + ArrayElemOffset(name, static_cast<int>(i));
        }
        rewriter.ReplaceText(param->getSourceRange(), text);
      }
    }
  }
}

void XilinxBackend::RewriteTaskFunc(const TaskModel& task, bool is_top,
                                    clang::Rewriter& rewriter) const {
  const clang::FunctionDecl* func = task.def;
  if (!func->hasBody()) return;
  const Lvl lvl = LvlOf(task.level, is_top, is_vitis_);
  const std::string lines = GeneratePreamble(*this, task, is_top);

  if (lvl == Lvl::kLower) {
    // Leading newline so the first pragma starts its own line after the brace.
    rewriter.InsertTextAfterToken(func->getBody()->getBeginLoc(), "\n" + lines);
    RewriteStreamDefinitions(func, rewriter);
    RemoveInline(func, rewriter);
    return;
  }

  // Middle level (and top level in non-Vitis mode): replace the body with a
  // shell carrying just the interface pragmas.
  if (lvl == Lvl::kMiddle || (lvl == Lvl::kTop && !is_vitis_)) {
    rewriter.ReplaceText(func->getBody()->getSourceRange(),
                         "{\n" + lines + "}\n");
    RemoveInline(func, rewriter);
    return;
  }

  // Top level in Vitis mode: the shell carries an s_axilite return-control
  // pragma, and every declaration (forward decl + definition) is wrapped in
  // extern "C" (Vitis only links extern-C kernels).
  const std::string shell =
      "{\n" + lines +
      "#pragma HLS interface s_axilite port = return bundle = control\n}\n";
  for (const clang::FunctionDecl* decl : func->redecls()) {
    clang::SourceLocation end = decl->getEndLoc();
    if (decl->isThisDeclarationADefinition()) {
      rewriter.ReplaceText(decl->getBody()->getSourceRange(), shell);
    } else {
      // Insert after the declaration's trailing semicolon.
      const clang::SourceLocation after = clang::Lexer::findLocationAfterToken(
          end, clang::tok::semi, rewriter.getSourceMgr(),
          rewriter.getLangOpts(), /*SkipTrailingWhitespaceAndNewLine=*/true);
      if (after.isValid()) end = after;
    }
    rewriter.InsertText(decl->getBeginLoc(), "extern \"C\" {\n\n");
    rewriter.InsertTextAfterToken(end, "\n\n}  // extern \"C\"\n");
    RemoveInline(decl, rewriter);
  }
}

void XilinxBackend::StripOtherTask(const clang::FunctionDecl* func,
                                   clang::Rewriter& rewriter) const {
  if (func->hasBody()) {
    rewriter.ReplaceText(func->getBody()->getSourceRange(), ";");
  }
}

void XilinxBackend::RewriteHelperFunc(const clang::FunctionDecl* func,
                                      clang::Rewriter& rewriter) const {
  // Non-task helpers keep their body; only stream declarations get depth
  // pragmas (the old RewriteOtherFunc emitted no per-parameter interface code
  // for Xilinx).
  RewriteStreamDefinitions(func, rewriter);
}

void XilinxBackend::LowerPipeline(int ii, const clang::Stmt* body,
                                  clang::Rewriter& rewriter) const {
  if (body == nullptr) return;
  std::string pragma = "HLS pipeline";
  if (ii != 0) pragma += " II = " + std::to_string(ii);
  AddPragmaToBody(rewriter, body, pragma);
}

void XilinxBackend::LowerUnroll(int factor, const clang::Stmt* body,
                                clang::Rewriter& rewriter) const {
  if (body == nullptr) return;
  std::string pragma = "HLS unroll";
  if (factor != 0) pragma += " factor = " + std::to_string(factor);
  AddPragmaToBody(rewriter, body, pragma);
}

}  // namespace tapa::cc
