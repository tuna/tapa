#include "emit.h"

#include <string>

#include "clang/AST/Stmt.h"

#include "../frontend/type_args.h"
#include "conventions.h"

namespace tapa::cc {

namespace {

// Element (0th template arg) type spelling of a stream/mmap, e.g. "float".
std::string ElementType(const clang::ParmVarDecl* param) {
  if (const auto* arg = GetTemplateArg(param->getType(), 0)) {
    return TemplateArgName(*arg);
  }
  return "";
}

// Element bit width, or 0.
uint32_t ElementWidth(const clang::ParmVarDecl* param) {
  if (const auto* arg = GetTemplateArg(param->getType(), 0)) {
    if (arg->getKind() == clang::TemplateArgument::Type) {
      return BitWidth(param->getASTContext(), arg->getAsType());
    }
  }
  return 0;
}

// Channel count of an array interface, or 0.
int64_t ArraySize(const clang::ParmVarDecl* param) {
  return IntTemplateArg(param->getType(), 1).value_or(0);
}

}  // namespace

void EmitDummyStreamRW(const clang::ParmVarDecl* param, TapaKind kind,
                       CodeSink& out, bool qdma) {
  const std::string name = param->getNameAsString();

  auto dummy_read = [&](const std::string& n) {
    out.Line("{ auto val = " + n + ".read(); }");
    if (!qdma) {  // non-qdma streams support peek
      out.Line("{ auto val = " + n + ".peek(nullptr); }");
    }
  };
  auto dummy_write = [&](const std::string& n, const std::string& type) {
    out.Line(n + ".write(" + type + "());");
  };

  switch (kind) {
    case TapaKind::kIStream:
      dummy_read(name);
      break;
    case TapaKind::kOStream: {
      std::string type = ElementType(param);
      if (qdma) {
        type =
            "qdma_axis<" + std::to_string(ElementWidth(param)) + ", 0, 0, 0>";
      }
      dummy_write(name, type);
      break;
    }
    case TapaKind::kIStreams:
      if (qdma) {
        out.Line("#error istreams not supported for qdma-based tasks");
      } else {
        for (int64_t i = 0; i < ArraySize(param); ++i) {
          dummy_read(ArrayNameAt(name, static_cast<int>(i)));
        }
      }
      break;
    case TapaKind::kOStreams:
      if (qdma) {
        out.Line("#error ostreams not supported for qdma-based tasks");
      } else {
        const std::string type = ElementType(param);
        for (int64_t i = 0; i < ArraySize(param); ++i) {
          dummy_write(ArrayNameAt(name, static_cast<int>(i)), type);
        }
      }
      break;
    default:
      break;
  }
}

void EmitDummyMmapOrScalarRW(const clang::ParmVarDecl* param, TapaKind kind,
                             CodeSink& out) {
  const std::string name = param->getNameAsString();
  if (kind == TapaKind::kMmaps || kind == TapaKind::kAsyncMmaps ||
      kind == TapaKind::kHmap) {
    for (int64_t i = 0; i < ArraySize(param); ++i) {
      out.Line("{ auto val = reinterpret_cast<volatile uint8_t&>(" +
               ArrayElemOffset(name, static_cast<int>(i)) + "); }");
    }
  } else {
    const std::string ref = kind == TapaKind::kMmap ? OffsetName(name) : name;
    const std::string qual =
        param->getType().isConstQualified() ? "const " : "";
    out.Line("{ auto val = reinterpret_cast<volatile " + qual + "uint8_t&>(" +
             ref + "); }");
  }
}

void AddPragmaToBody(clang::Rewriter& rewriter, const clang::Stmt* body,
                     const std::string& pragma) {
  if (const auto* compound = llvm::dyn_cast<clang::CompoundStmt>(body)) {
    rewriter.InsertTextAfterToken(compound->getLBracLoc(),
                                  "\n#pragma " + pragma + "\n");
  } else {
    rewriter.InsertTextBefore(body->getBeginLoc(),
                              "_Pragma(\"" + pragma + "\")");
  }
}

void AddPragmaAfterStmt(clang::Rewriter& rewriter, const clang::Stmt* stmt,
                        const std::string& pragma) {
  rewriter.InsertTextAfterToken(stmt->getEndLoc(),
                                "\n#pragma " + pragma + "\n");
}

}  // namespace tapa::cc
