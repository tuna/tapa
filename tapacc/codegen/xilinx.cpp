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
#include "edit_sink.h"
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

// HLS interface pragmas for the FIFO(s) of the stream port `name`: one, or
// `channels` of an istreams/ostreams port.
void EmitLowerStream(const std::string& name, TapaKind kind, int64_t channels,
                     CodeSink& out) {
  out.Pragma({"HLS disaggregate variable =", name});

  std::vector<std::string> names;
  if (kind == TapaKind::kIStreams || kind == TapaKind::kOStreams) {
    out.Pragma({"HLS array_partition variable =", name, "complete"});
    for (int64_t i = 0; i < channels; ++i) {
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

// The bank port and token FIFOs of the frame port `name` (see `tapa_frames`
// in tapa/xilinx/hls/stream.h): one memory port, read for an iframes port and
// written for an oframes port, of latency 1, which the frame channel serves.
// The token streams of integers are ap_fifo ports without a pragma.
void EmitLowerFrames(const std::string& name, CodeSink& out) {
  out.Pragma({"HLS disaggregate variable =", name});
  out.Pragma({"HLS interface ap_memory port =", name + "._buf",
              "storage_type = ram_1p latency = 1"});
  out.Pragma({"HLS aggregate variable =", name + "._buf", "bit"});
  out.Line("void(" + name + "._acquire.empty());");
  out.Line("void(" + name + "._release.full());");
}

// The five FIFOs of the async_mmap port `name`.
void EmitLowerAsyncMmap(const std::string& name, CodeSink& out) {
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

// Add `#pragma HLS stream` depth pragmas after each stream declaration.
// Walks the whole body: multi-declarator statements contribute one pragma
// per declarator, and streams declared in nested scopes get their depth
// too; a shallower walk would silently leave both at HLS's default depth.
void RewriteStreamDefinitions(const clang::Stmt* stmt, EditSink& edits) {
  if (stmt == nullptr) return;
  if (const auto* decl_stmt = llvm::dyn_cast<clang::DeclStmt>(stmt)) {
    for (const clang::Decl* d : decl_stmt->decls()) {
      const auto* var = llvm::dyn_cast<clang::VarDecl>(d);
      if (var == nullptr) continue;
      if (ClassifyTapaType(var->getType()) == TapaKind::kStream) {
        const int64_t depth = IntTemplateArg(var->getType(), 1).value_or(0);
        AddPragmaAfterStmt(edits, decl_stmt,
                           "HLS stream variable = " + var->getNameAsString() +
                               "._ depth = " + std::to_string(depth));
      }
    }
    return;
  }
  for (const clang::Stmt* child : stmt->children()) {
    RewriteStreamDefinitions(child, edits);
  }
}

void RewriteStreamDefinitions(const clang::FunctionDecl* func,
                              EditSink& edits) {
  if (!func->hasBody()) return;
  RewriteStreamDefinitions(func->getBody(), edits);
}

}  // namespace

void XilinxBackend::EmitStreamPort(const PortContext& p, CodeSink& out) const {
  const Lvl lvl = LvlOf(p.level, p.is_top, is_vitis_);
  if (lvl == Lvl::kTop && p.ring) {
    // The ring's base address; its counters follow the last parameter.
    const std::string offset = OffsetName(p.param->getNameAsString());
    out.Pragma({"HLS interface s_axilite port =", offset, "bundle = control"});
    out.Line("{ auto val = reinterpret_cast<volatile uint8_t&>(" + offset +
             "); }");
    return;
  }
  if (lvl == Lvl::kTop) {
    out.Pragma({"HLS interface axis port =", p.param->getNameAsString()});
    EmitDummyStreamRW(p.param, p.kind, out, /*qdma=*/true);
    return;
  }
  if (IsFramesPort(p.kind)) {  // only on a leaf: tapacc rejects the rest
    EmitLowerFrames(p.param->getNameAsString(), out);
    return;
  }
  const bool plural =
      p.kind == TapaKind::kIStreams || p.kind == TapaKind::kOStreams;
  EmitLowerStream(p.param->getNameAsString(), p.kind,
                  plural ? ArraySize(p.param) : 1, out);
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
    EmitLowerAsyncMmap(p.param->getNameAsString(), out);
  } else {
    // Middle/top async_mmap is handled like a scalar offset register.
    EmitScalarPort(p, out);
  }
}

std::string XilinxBackend::GenerateMover(const Mover& mover) const {
  // The ports MoverPorts lists: a write request and its data share the
  // request channel.
  const std::string& t = mover.element;
  std::string params = "tapa::async_mmap<" + t + ">& mem";
  std::string call;
  CodeSink sink;
  EmitLowerAsyncMmap("mem", sink);
  EmitLowerStream("requests", TapaKind::kIStream, 1, sink);
  if (!mover.writes) {
    params += ", tapa::istream<tapa::burst>& requests";
    params += ", tapa::ostream<" + t + ">& data";
    call = "tapa::internal::serve_reads(mem, requests, data";
    EmitLowerStream("data", TapaKind::kOStream, 1, sink);
  } else {
    params += ", tapa::istream<" + t + ">& requests";
    call = "tapa::internal::serve_writes(mem, requests";
    if (mover.responses) {
      params += ", tapa::ostream<uint32_t>& responses";
      call += ", responses";
      EmitLowerStream("responses", TapaKind::kOStream, 1, sink);
    }
  }
  std::string code = "\n\nvoid " + mover.name + "(" + params + ") {\n";
  for (const std::string& line : sink.Lines()) {
    code += "  " + line + "\n";
  }
  code += "  " + call + ");\n}\n";
  return code;
}

void XilinxBackend::EmitScalarPort(const PortContext& p, CodeSink& out) const {
  const Lvl lvl = LvlOf(p.level, p.is_top, is_vitis_);
  const std::string name = p.param->getNameAsString();
  switch (lvl) {
    case Lvl::kLower:
      break;  // no interface pragma for lower-level scalars
    case Lvl::kMiddle:
    case Lvl::kTop:
      if (lvl == Lvl::kTop) {
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
                                     EditSink& edits) const {
  const Lvl lvl = LvlOf(task.level, is_top, is_vitis_);
  if (lvl == Lvl::kLower) return;  // lower-level signatures are unchanged

  // Middle/top: replace mmap parameters with 64-bit base addresses, on every
  // declaration of the function (the forward declaration and the definition),
  // so the emitted signatures agree.
  for (const clang::FunctionDecl* decl : task.def->redecls()) {
    bool rewrote_axis_stream = false;
    std::string counters;
    for (const clang::ParmVarDecl* param : decl->parameters()) {
      const std::string name = param->getNameAsString();
      const TapaKind kind = ClassifyTapaType(param);
      if (lvl == Lvl::kTop && task.rings.count(name) != 0) {
        // A ring: its base address in place, its registers after the last
        // parameter, so the host's argument indices stay the parameters'.
        edits.ReplaceText(
            param->getTypeSourceInfo()->getTypeLoc().getSourceRange(),
            "uint64_t");
        edits.ReplaceText(param->getLocation(), OffsetName(name));
        counters += ", uint32_t " + RingHostFramesName(name) + ", uint32_t& " +
                    RingKernelFramesName(name) + ", uint64_t& " +
                    RingShapeName(name);
      } else if (kind == TapaKind::kMmap || kind == TapaKind::kAsyncMmap) {
        edits.ReplaceText(
            param->getTypeSourceInfo()->getTypeLoc().getSourceRange(),
            "uint64_t");
        edits.ReplaceText(param->getLocation(), OffsetName(name));
      } else if (IsMmapArray(kind)) {
        std::string text;
        for (int64_t i = 0; i < ArraySize(param); ++i) {
          if (!text.empty()) text += ", ";
          text += "uint64_t " + ArrayElemOffset(name, static_cast<int>(i));
        }
        edits.ReplaceText(param->getSourceRange(), text);
      } else if (lvl == Lvl::kTop &&
                 (kind == TapaKind::kIStream || kind == TapaKind::kOStream)) {
        // Vitis top-level streams become axis interfaces. Rewrite the type to
        // hls::stream<qdma_axis<W>> so Vitis HLS emits `{name}_TDATA/_TVALID/
        // _TREADY/_TLAST` (what the cosim testbench binds to), instead of the
        // `tapa::istream` disaggregated `_s_*` member ports.
        if (const auto* arg = GetTemplateArg(param->getType(), 0)) {
          if (arg->getKind() == clang::TemplateArgument::Type) {
            const uint32_t w = AxisDataWidth(param);
            edits.ReplaceText(
                param->getTypeSourceInfo()->getTypeLoc().getSourceRange(),
                "hls::stream<qdma_axis<" + std::to_string(w) + ", 0, 0, 0> >&");
            rewrote_axis_stream = true;
          }
        }
      }
    }
    // A declaration can live in a header included before the definition.
    // Make qdma_axis available at each rewritten declaration, not just in
    // the translation unit containing the function body.
    if (rewrote_axis_stream) {
      edits.InsertTextAfter(decl->getBeginLoc(),
                            "#include \"ap_axi_sdata.h\"\n"
                            "#include \"hls_stream.h\"\n\n");
    }
    if (!counters.empty()) {
      edits.InsertTextBefore(decl->getFunctionTypeLoc().getRParenLoc(),
                             counters);
    }
  }
}

void XilinxBackend::WrapTaskDeclExternC(const clang::FunctionDecl* decl,
                                        EditSink& edits) const {
  // Anchor at the first character after the declaration's semicolon. Using
  // that character as a token anchor could consume the next declaration.
  const clang::SourceLocation after_semi = clang::Lexer::findLocationAfterToken(
      decl->getEndLoc(), clang::tok::semi, edits.getSourceMgr(),
      edits.getLangOpts(), /*SkipTrailingWhitespaceAndNewLine=*/false);
  edits.InsertTextAfter(decl->getBeginLoc(), "extern \"C\" {\n\n");
  edits.InsertTextBefore(after_semi, "\n\n}  // extern \"C\"\n");
  RemoveInline(decl, edits);
}

void XilinxBackend::RewriteTaskFunc(const TaskModel& task, bool is_top,
                                    EditSink& edits) const {
  const clang::FunctionDecl* func = task.def;
  const Lvl lvl = LvlOf(task.level, is_top, is_vitis_);
  if (!func->hasBody()) {
    // A TU holding only declarations of the task (the shared header
    // announcing a task another TU defines) still takes the one
    // declaration-level rewrite the defining TU applies to that same
    // declaration through its redeclaration chain; without it the header
    // would rewrite differently in the two translation units including it.
    if (lvl == Lvl::kTop) WrapTaskDeclExternC(func, edits);
    return;
  }
  CodeSink sink;
  EmitPortPreambles(func, task.level, is_top, sink, &task.rings);
  if (lvl == Lvl::kTop) {
    for (const auto& [name, slots] : task.rings) {
      const std::string host = RingHostFramesName(name);
      const std::string kernel = RingKernelFramesName(name);
      sink.Pragma({"HLS interface s_axilite port =", host, "bundle = control"});
      sink.Line("{ auto val = reinterpret_cast<volatile uint8_t&>(" + host +
                "); }");
      sink.Pragma(
          {"HLS interface s_axilite port =", kernel, "bundle = control"});
      sink.Line(kernel + " = 0;");
      const std::string shape = RingShapeName(name);
      sink.Pragma(
          {"HLS interface s_axilite port =", shape, "bundle = control"});
      sink.Line(shape + " = 0;");
      sink.Line("");
    }
  }
  const std::string lines = sink.Str();

  if (lvl == Lvl::kLower) {
    // Leading newline so the first pragma starts its own line after the brace.
    edits.InsertTextAfterToken(func->getBody()->getBeginLoc(), "\n" + lines);
    RewriteStreamDefinitions(func, edits);
    RemoveInline(func, edits);
    return;
  }

  // Middle level (and top level in non-Vitis mode, which LvlOf maps to
  // kMiddle): replace the body with a shell carrying just the interface
  // pragmas.
  if (lvl == Lvl::kMiddle) {
    edits.ReplaceText(func->getBody()->getSourceRange(), "{\n" + lines + "}\n");
    RemoveInline(func, edits);
    return;
  }

  // Top level in Vitis mode: the shell carries an s_axilite return-control
  // pragma, and every declaration (forward decl + definition) is wrapped in
  // extern "C" (Vitis only links extern-C kernels).
  const std::string shell =
      "{\n" + lines +
      "#pragma HLS interface s_axilite port = return bundle = control\n}\n";
  for (const clang::FunctionDecl* decl : func->redecls()) {
    if (decl->isThisDeclarationADefinition()) {
      edits.ReplaceText(decl->getBody()->getSourceRange(), shell);
      edits.InsertTextAfter(decl->getBeginLoc(), "extern \"C\" {\n\n");
      edits.InsertTextAfterToken(decl->getEndLoc(), "\n\n}  // extern \"C\"\n");
      RemoveInline(decl, edits);
    } else {
      WrapTaskDeclExternC(decl, edits);
    }
  }
}

void XilinxBackend::StripOtherTask(const clang::FunctionDecl* func,
                                   EditSink& edits) const {
  if (func->hasBody()) {
    edits.ReplaceText(func->getBody()->getSourceRange(), ";");
  }
}

void XilinxBackend::RewriteHelperFunc(const clang::FunctionDecl* func,
                                      EditSink& edits) const {
  // Only definitions are rewritten: a redeclaration shares the
  // definition's body (`hasBody()` looks through the chain), so rewriting
  // one too would insert every stream pragma a second time at the same
  // point, and the redeclaration's own inline keyword would emit the
  // opposite pragma at the definition.
  if (!func->isThisDeclarationADefinition()) return;
  // Non-task helpers keep their body; only stream declarations get depth
  // pragmas.
  RewriteStreamDefinitions(func, edits);
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
    edits.InsertTextBefore(insert, "inline ");
  }
  edits.InsertTextBefore(insert, is_inline ? " __attribute__((always_inline)) "
                                           : " __attribute__((noinline)) ");
  // A body written in a macro cannot be rewritten (the pragma would land in
  // the expansion); the attribute pair still applies.
  const clang::SourceManager& sm = edits.getSourceMgr();
  const clang::Stmt* body = func->getBody();
  if (sm.isMacroBodyExpansion(body->getBeginLoc())) {
    llvm::errs() << "Warning: helper " << func->getNameAsString()
                 << " has a macro body; inline pragma not emitted\n";
    return;
  }
  AddPragmaToBody(edits, body, is_inline ? "HLS inline" : "HLS inline off");
}

void XilinxBackend::LowerPipeline(int ii, const std::string& style,
                                  const clang::Stmt* body,
                                  EditSink& edits) const {
  if (body == nullptr) return;
  std::string pragma = "HLS pipeline";
  // An explicit 0 disables pipelining, mirroring `flatten(false)`.
  if (ii == 0) {
    AddPragmaToBody(edits, body, "HLS pipeline off");
    return;
  }
  // stp = stable (flushable) pipeline, flp = free-running: a real
  // scheduling-semantics difference, never dropped.
  if (!style.empty()) pragma += " style = " + style;
  // The omitted sentinel (0xFFFFFFFF) arrives here as -1, leaving the
  // vendor its default initiation interval.
  if (ii > 0) pragma += " II = " + std::to_string(ii);
  AddPragmaToBody(edits, body, pragma);
}

void XilinxBackend::LowerUnroll(int factor, const clang::Stmt* body,
                                EditSink& edits) const {
  if (body == nullptr) return;
  std::string pragma = "HLS unroll";
  if (factor != 0) pragma += " factor = " + std::to_string(factor);
  AddPragmaToBody(edits, body, pragma);
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
std::string FormatRegionPragma(const clang::Attr& attr, EditSink& edits) {
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
      .Case([&edits](const clang::TapaDependenceAttr* attr) {
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
                            edits.getSourceMgr(), edits.getLangOpts())
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
                                  EditSink& edits) const {
  if (body == nullptr) return;
  const std::string pragma = FormatRegionPragma(attr, edits);
  if (!pragma.empty()) AddPragmaToBody(edits, body, pragma);
}

void XilinxBackend::LowerDeclAttr(const clang::Attr& attr,
                                  const clang::VarDecl& var,
                                  const clang::DeclStmt& decl,
                                  EditSink& edits) const {
  const std::string name = var.getNameAsString();
  // Region attributes that landed on a declaration (they sat before a
  // declaration statement): same pragma text, placed after the declaration.
  if (const std::string region = FormatRegionPragma(attr, edits);
      !region.empty()) {
    AddPragmaAfterStmt(edits, &decl, region);
    return;
  }
  const std::string pragma = FormatDeclPragma(attr, name);
  if (!pragma.empty()) AddPragmaAfterStmt(edits, &decl, pragma);
}

// Function-parameter arrays have no declaration statement: the pragma goes
// at the top of the body, like the vendor pragma the attribute replaces.
void XilinxBackend::LowerParamAttr(const clang::Attr& attr,
                                   const clang::ParmVarDecl& param,
                                   const clang::Stmt* body,
                                   EditSink& edits) const {
  if (body == nullptr) return;
  const std::string pragma = FormatDeclPragma(attr, param.getNameAsString());
  if (!pragma.empty()) AddPragmaToBody(edits, body, pragma);
}
}  // namespace tapa::cc
