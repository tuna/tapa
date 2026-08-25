#include "emit.h"

#include <cctype>
#include <string>

#include "clang/AST/Stmt.h"
#include "clang/Lex/Lexer.h"

#include "conventions.h"
#include "frontend/type_args.h"

namespace tapa::cc {

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
      std::string type = ElementTypeName(param);
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
        const std::string type = ElementTypeName(param);
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
  if (kind == TapaKind::kMmaps || kind == TapaKind::kHmap) {
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

void RemoveLoweredAttr(clang::Rewriter& rewriter,
                       clang::SourceRange attr_range) {
  auto begin = attr_range.getBegin();
  auto end = attr_range.getEnd();
  auto at = [&](clang::SourceLocation a, clang::SourceLocation b) {
    return rewriter.getRewrittenText(clang::SourceRange(a, b));
  };
  auto is_space = [&](const std::string& s) {
    return s.empty() || std::isspace(static_cast<unsigned char>(s[0]));
  };
  auto is_alpha = [&](const std::string& s) {
    return !s.empty() && std::isalpha(static_cast<unsigned char>(s[0]));
  };

  // Find the true end of the token.
  for (; is_alpha(at(end.getLocWithOffset(1), end.getLocWithOffset(1)));
       end = end.getLocWithOffset(1)) {
  }
  // Swallow surrounding whitespace.
  for (; is_space(at(begin.getLocWithOffset(-1), begin.getLocWithOffset(-1)));
       begin = begin.getLocWithOffset(-1)) {
  }
  for (; is_space(at(end.getLocWithOffset(1), end.getLocWithOffset(1)));
       end = end.getLocWithOffset(1)) {
  }
  // Swallow a neighbouring comma, or an enclosing [[ ]].
  if (at(begin.getLocWithOffset(-1), begin.getLocWithOffset(-1)) == ",") {
    begin = begin.getLocWithOffset(-1);
  } else if (at(end.getLocWithOffset(1), end.getLocWithOffset(1)) == ",") {
    end = end.getLocWithOffset(1);
  } else if (at(begin.getLocWithOffset(-2), begin.getLocWithOffset(-1)) ==
                 "[[" &&
             at(end.getLocWithOffset(1), end.getLocWithOffset(2)) == "]]") {
    begin = begin.getLocWithOffset(-2);
    end = end.getLocWithOffset(2);
  }
  rewriter.RemoveText(clang::SourceRange(begin, end));
}

}  // namespace tapa::cc
