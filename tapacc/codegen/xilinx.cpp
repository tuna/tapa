#include "xilinx.h"

#include <string>
#include <vector>

#include "clang/AST/Attr.h"
#include "clang/AST/Decl.h"
#include "clang/AST/DeclTemplate.h"
#include "clang/AST/Stmt.h"
#include "clang/AST/TypeLoc.h"
#include "clang/Basic/SourceManager.h"
#include "clang/Lex/Lexer.h"
#include "llvm/ADT/TypeSwitch.h"
#include "llvm/Support/raw_ostream.h"

#include "conventions.h"
#include "emit.h"
#include "frontend/classify.h"
#include "frontend/type_args.h"

namespace tapa::cc {

namespace {

// The effective code-generation level of a function. The top-level distinction
// only exists in Vitis mode; in plain Vitis HLS the top task is generated as a
// middle-level upper task.
enum class Lvl { kTop, kMiddle, kLower };

Lvl LvlOf(TaskLevel level, bool is_top, bool is_vitis) {
  if (level == TaskLevel::kLower) return Lvl::kLower;
  if (is_top && is_vitis) return Lvl::kTop;
  return Lvl::kMiddle;
}

bool IsMmapArray(TapaKind k) {
  return k == TapaKind::kMmaps || k == TapaKind::kHmap;
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

// Add `#pragma HLS stream` depth pragmas after each stream declaration.
// Walks the whole body: multi-declarator statements contribute one pragma
// per declarator, and streams declared in nested scopes get their depth
// too (both previously skipped silently, leaving HLS's default depth).
void RewriteStreamDefinitions(const clang::Stmt* stmt,
                              clang::Rewriter& rewriter) {
  if (stmt == nullptr) return;
  if (const auto* decl_stmt = llvm::dyn_cast<clang::DeclStmt>(stmt)) {
    for (const clang::Decl* d : decl_stmt->decls()) {
      const auto* var = llvm::dyn_cast<clang::VarDecl>(d);
      if (var == nullptr) continue;
      if (ClassifyTapaType(var->getType()) == TapaKind::kStream) {
        const int64_t depth = IntTemplateArg(var->getType(), 1).value_or(0);
        AddPragmaAfterStmt(rewriter, decl_stmt,
                           "HLS stream variable = " + var->getNameAsString() +
                               "._ depth = " + std::to_string(depth));
      }
    }
    return;
  }
  for (const clang::Stmt* child : stmt->children()) {
    RewriteStreamDefinitions(child, rewriter);
  }
}

void RewriteStreamDefinitions(const clang::FunctionDecl* func,
                              clang::Rewriter& rewriter) {
  if (!func->hasBody()) return;
  RewriteStreamDefinitions(func->getBody(), rewriter);
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
    // Middle/top async_mmap is handled like a scalar offset register.
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
    case Lvl::kTop:
      if (lvl == Lvl::kTop && is_vitis_) {
        out.Pragma(
            {"HLS interface s_axilite port =", name, "bundle = control"});
      } else {
        out.Pragma({"HLS interface ap_none port =", name, "register"});
      }
      EmitDummyMmapOrScalarRW(p.param, p.kind, out);
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
  bool rewrote_axis_stream = false;
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
      } else if (lvl == Lvl::kTop &&
                 (kind == TapaKind::kIStream || kind == TapaKind::kOStream)) {
        // Vitis top-level streams become axis interfaces. Rewrite the type to
        // hls::stream<qdma_axis<W>> so Vitis HLS emits `{name}_TDATA/_TVALID/
        // _TREADY/_TLAST` (what the cosim testbench binds to), instead of the
        // `tapa::istream` disaggregated `_s_*` member ports.
        if (const auto* arg = GetTemplateArg(param->getType(), 0)) {
          if (arg->getKind() == clang::TemplateArgument::Type) {
            const uint32_t w =
                param->getASTContext().getTypeInfo(arg->getAsType()).Width;
            rewriter.ReplaceText(
                param->getTypeSourceInfo()->getTypeLoc().getSourceRange(),
                "hls::stream<qdma_axis<" + std::to_string(w) + ", 0, 0, 0> >&");
            rewrote_axis_stream = true;
          }
        }
      }
    }
  }

  // The rewritten signature (and the dummy writes in the top task's body) spell
  // qdma_axis<...>, whose definition lives in ap_axi_sdata.h. This runs once
  // per emitted cpp (RewriteSignature is called for the top task in every
  // file), so every file that carries the rewritten decl gets the header.
  if (rewrote_axis_stream) {
    rewriter.InsertText(task.def->getBeginLoc(),
                        "#include \"ap_axi_sdata.h\"\n"
                        "#include \"hls_stream.h\"\n\n",
                        /*InsertAfter=*/true);
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
  // Only definitions are rewritten: a redeclaration shares the
  // definition's body (`hasBody()` looks through the chain), so rewriting
  // one too would insert every stream pragma a second time at the same
  // point, and the redeclaration's own inline keyword would emit the
  // opposite pragma at the definition.
  if (!func->isThisDeclarationADefinition()) return;
  // Non-task helpers keep their body; only stream declarations get depth
  // pragmas (the old RewriteOtherFunc emitted no per-parameter interface code
  // for Xilinx).
  RewriteStreamDefinitions(func, rewriter);
  // The C++ inline keyword is the portable inlining control: `inline` means
  // always inline, no keyword means never inline. Emit the vendor pragma AND
  // the standard clang attribute as a consistent pair — the pragma is what
  // the tool consumes today, the attribute pins the decision in the front
  // end so a vendor silently dropping the pragma cannot flip hierarchy.
  // (always_inline legally requires the inline keyword; both branches
  // satisfy their precondition by construction.)
  // Policy spans the whole redeclaration chain (`inline` on any declaration
  // makes the function inline).
  clang::SourceLocation insert = func->getBeginLoc();
  if (const auto* tp = func->getDescribedFunctionTemplate()) {
    // The attribute must follow the template header, not precede it.
    insert = tp->getTemplateParameters()->getRAngleLoc().getLocWithOffset(1);
  } else if (func->getTemplateSpecializationInfo() != nullptr) {
    // An explicit specialization: attributes may not precede the
    // `template <>` introducer; insert at the decl-specifier start.
    insert = func->getInnerLocStart();
  }
  const bool is_inline = func->isInlined();
  if (is_inline && !func->isInlineSpecified()) {
    // always_inline requires the keyword on this declaration; the chain may
    // carry it on an earlier one.
    rewriter.InsertTextBefore(insert, "inline ");
  }
  rewriter.InsertTextBefore(insert, is_inline
                                        ? " __attribute__((always_inline)) "
                                        : " __attribute__((noinline)) ");
  // A body written in a macro cannot be rewritten (the pragma would land in
  // the expansion); the attribute pair still applies.
  const clang::SourceManager& sm = rewriter.getSourceMgr();
  const clang::Stmt* body = func->getBody();
  if (sm.isMacroBodyExpansion(body->getBeginLoc())) {
    llvm::errs() << "Warning: helper " << func->getNameAsString()
                 << " has a macro body; inline pragma not emitted\n";
    return;
  }
  AddPragmaToBody(rewriter, body, is_inline ? "HLS inline" : "HLS inline off");
}

void XilinxBackend::LowerPipeline(int ii, const std::string& style,
                                  const clang::Stmt* body,
                                  clang::Rewriter& rewriter) const {
  if (body == nullptr) return;
  std::string pragma = "HLS pipeline";
  // An explicit 0 disables pipelining, mirroring `flatten(false)`; the
  // omitted sentinel leaves the vendor its default initiation interval.
  if (ii == 0) {
    AddPragmaToBody(rewriter, body, "HLS pipeline off");
    return;
  }
  // stp = stable (flushable) pipeline, flp = free-running: a real
  // scheduling-semantics difference, never dropped.
  if (!style.empty()) pragma += " style = " + style;
  // The omitted sentinel (0xFFFFFFFF) arrives here as -1, leaving the
  // vendor its default initiation interval.
  if (ii > 0) pragma += " II = " + std::to_string(ii);
  AddPragmaToBody(rewriter, body, pragma);
}

void XilinxBackend::LowerUnroll(int factor, const clang::Stmt* body,
                                clang::Rewriter& rewriter) const {
  if (body == nullptr) return;
  std::string pragma = "HLS unroll";
  if (factor != 0) pragma += " factor = " + std::to_string(factor);
  AddPragmaToBody(rewriter, body, pragma);
}

namespace {

// 0xFFFFFFFF (-1) is the "omitted" sentinel for optional integers whose
// zero is meaningful (array_map offset, storage/bind_op latency).
bool IsOmitted(uint32_t value) { return value == 0xFFFFFFFF; }

// tripcount/latency bounds are REQUIRED attribute arguments, and zero is a
// meaningful bound (e.g. `latency max = 0` = combinational): always emit.
std::string FormatBounds(std::string pragma, int min, int max) {
  pragma.append(" min = ").append(std::to_string(min));
  pragma.append(" max = ").append(std::to_string(max));
  return pragma;
}

// The single formatting home for region attributes, shared by the
// statement and declaration lowering paths.
std::string FormatRegionPragma(const clang::Attr& attr,
                               clang::Rewriter& rewriter) {
  return llvm::TypeSwitch<const clang::Attr*, std::string>(&attr)
      .Case([](const clang::TapaTripcountAttr* attr) {
        return FormatBounds("HLS loop_tripcount", attr->getMin(),
                            attr->getMax());
      })
      .Case([](const clang::TapaFlattenAttr* attr) {
        return attr->getEnable() == 0 ? std::string{"HLS loop_flatten off"}
                                      : std::string{"HLS loop_flatten"};
      })
      .Case([](const clang::TapaLatencyAttr* attr) {
        return FormatBounds("HLS latency", attr->getMin(), attr->getMax());
      })
      .Case([&rewriter](const clang::TapaDependenceAttr* attr) {
        // The vendor's keyword form (accepted since classic vitis_hls, and
        // what current Vitis documents). Dependent = a real dependence
        // assertion at `distance`; the default asserts independence.
        std::string pragma =
            "HLS dependence variable = " + attr->getVariable().str();
        if (!attr->getClassName().empty())
          pragma += " class = " + attr->getClassName().str();
        if (!attr->getType().empty())
          pragma += " type = " + attr->getType().str();
        if (!attr->getDirection().empty())
          pragma += " direction = " + attr->getDirection().str();
        pragma += attr->getDependent() != 0 ? " dependent = true"
                                            : " dependent = false";
        // Distance keeps the user's spelling: the macro name, the constant,
        // or the string's content.
        if (attr->getDependent() != 0) {
          if (const clang::Expr* d = attr->getDistance()) {
            const clang::Expr* e = d->IgnoreImpCasts();
            if (const auto* sl = llvm::dyn_cast<clang::StringLiteral>(e)) {
              pragma += " distance = " + sl->getString().str();
            } else {
              pragma += " distance = " +
                        clang::Lexer::getSourceText(
                            clang::CharSourceRange::getTokenRange(
                                d->getSourceRange()),
                            rewriter.getSourceMgr(), rewriter.getLangOpts())
                            .str();
            }
          }
        }
        return pragma;
      })
      .Case([](const clang::TapaBalanceAttr*) {
        return std::string{"HLS expression_balance"};
      })
      .Default(std::string{});
}

// One formatting home for variable-targeted pragmas (declarations and
// function parameters share it): formats the vendor pragma naming @p name.
std::string FormatDeclPragma(const clang::Attr& attr, const std::string& name) {
  return llvm::TypeSwitch<const clang::Attr*, std::string>(&attr)
      .Case([&](const clang::TapaPartitionAttr* attr) {
        std::string pragma =
            "HLS array_partition variable = " + name + " type = " +
            clang::TapaPartitionAttr::ConvertPartTypeToStr(attr->getType());
        if (!IsOmitted(attr->getFactor()))
          pragma += " factor = " + std::to_string(int(attr->getFactor()));
        if (!IsOmitted(attr->getDim()))
          pragma += " dim = " + std::to_string(int(attr->getDim()));
        return pragma;
      })
      .Case([&](const clang::TapaStorageAttr* attr) {
        std::string pragma = "HLS bind_storage variable = " + name;
        if (!attr->getType().empty())
          pragma += " type = " + attr->getType().str();
        if (!attr->getImpl().empty())
          pragma += " impl = " + attr->getImpl().str();
        if (!IsOmitted(attr->getLatency()))
          pragma += " latency = " + std::to_string(int(attr->getLatency()));
        return pragma;
      })
      .Case([&](const clang::TapaAggregateAttr*) {
        return "HLS aggregate variable = " + name;
      })
      .Case([&](const clang::TapaArrayMapAttr* attr) {
        std::string pragma = "HLS array_map variable = " + name +
                             " instance = " + attr->getInstance().str();
        if (!IsOmitted(attr->getOffset()))
          pragma += " offset = " + std::to_string(int(attr->getOffset()));
        if (!attr->getOrient().empty())
          pragma += " " + attr->getOrient().str();  // horizontal|vertical
        return pragma;
      })
      .Case([&](const clang::TapaBindOpAttr* attr) {
        std::string pragma = "HLS bind_op variable = " + name +
                             " op = " + attr->getOp().str() +
                             " impl = " + attr->getImpl().str();
        if (!IsOmitted(attr->getLatency()))
          pragma += " latency = " + std::to_string(int(attr->getLatency()));
        return pragma;
      })
      .Default(std::string{});
}

}  // namespace

void XilinxBackend::LowerStmtAttr(const clang::Attr& attr,
                                  const clang::Stmt* body,
                                  clang::Rewriter& rewriter) const {
  if (body == nullptr) return;
  const std::string pragma = FormatRegionPragma(attr, rewriter);
  if (!pragma.empty()) AddPragmaToBody(rewriter, body, pragma);
}

void XilinxBackend::LowerDeclAttr(const clang::Attr& attr,
                                  const clang::VarDecl& var,
                                  const clang::DeclStmt& decl,
                                  clang::Rewriter& rewriter) const {
  const std::string name = var.getNameAsString();
  // Region attributes that landed on a declaration (they sat before a
  // declaration statement): same pragma text, placed after the declaration.
  if (const std::string region = FormatRegionPragma(attr, rewriter);
      !region.empty()) {
    AddPragmaAfterStmt(rewriter, &decl, region);
    return;
  }
  const std::string pragma = FormatDeclPragma(attr, name);
  if (!pragma.empty()) AddPragmaAfterStmt(rewriter, &decl, pragma);
}

// Function-parameter arrays have no declaration statement: the pragma goes
// at the top of the body, like the vendor pragma the attribute replaces.
void XilinxBackend::LowerParamAttr(const clang::Attr& attr,
                                   const clang::ParmVarDecl& param,
                                   const clang::Stmt* body,
                                   clang::Rewriter& rewriter) const {
  if (body == nullptr) return;
  const std::string pragma = FormatDeclPragma(attr, param.getNameAsString());
  if (!pragma.empty()) AddPragmaToBody(rewriter, body, pragma);
}
}  // namespace tapa::cc
