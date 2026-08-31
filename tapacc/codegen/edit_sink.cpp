#include "edit_sink.h"

#include <utility>

#include "clang/Basic/Diagnostic.h"
#include "clang/Lex/Lexer.h"

#include "frontend/diag.h"

namespace tapa::cc {

EditSink::EditSink(clang::ASTContext& ctx, clang::Rewriter& rewriter)
    : ctx_(&ctx), rewriter_(rewriter) {}

void EditSink::TrackFile(clang::FileID file, Resnap resnap) {
  resnaps_[file] = std::move(resnap);
}

bool EditSink::CanRewrite(clang::SourceRange range) {
  return CanEdit(range.getBegin(), range.getEnd());
}

bool EditSink::CanRewrite(clang::CharSourceRange range) {
  return CanEdit(range.getBegin(), range.getEnd());
}

// A file location must name a mirrored file; the splice of a macro-owned
// edit lands in its invocation's spelled file, which is under the same
// rule as a direct edit.
bool EditSink::Mirrored(clang::SourceLocation loc) {
  clang::SourceManager& sm = rewriter_.getSourceMgr();
  if (resnaps_.count(sm.getFileID(loc)) != 0) return true;
  ReportCustomDiag(*ctx_, clang::DiagnosticsEngine::Error, loc,
                   "cannot rewrite %0 in non-mirrored file '%1'")
      << construct_ << sm.getFilename(loc);
  return false;
}

// The splice a macro-owned edit at `loc` composes into. The residual
// impossible cases each get one precise diagnostic.
ExpansionSplice* EditSink::MacroSide(clang::SourceLocation loc) {
  if (splices_ == nullptr) {
    ReportCustomDiag(*ctx_, clang::DiagnosticsEngine::Error, loc,
                     "cannot rewrite %0 inside a macro expansion: this run "
                     "recorded no token stream to expand it through")
        << construct_;
    return nullptr;
  }
  std::string why;
  ExpansionSplice* splice = splices_->SpliceFor(loc, &why);
  if (splice == nullptr) {
    ReportSpliceReject(loc, why);
    return nullptr;
  }
  return Mirrored(splice->invocation().getBegin()) ? splice : nullptr;
}

// The one splice an edit over [begin, end] composes into: the whole range
// must live in one invocation. A range crossing the file/expansion
// boundary cannot be replaced through either home.
ExpansionSplice* EditSink::MacroHome(clang::SourceLocation begin,
                                     clang::SourceLocation end) {
  if (begin.isMacroID() != end.isMacroID()) {
    ReportCustomDiag(*ctx_, clang::DiagnosticsEngine::Error,
                     begin.isMacroID() ? begin : end,
                     "cannot rewrite %0: its range spans from user source "
                     "into a macro expansion")
        << construct_;
    return nullptr;
  }
  return MacroSide(begin.isMacroID() ? begin : end);
}

bool EditSink::CanEdit(clang::SourceLocation begin, clang::SourceLocation end) {
  if (ctx_ == nullptr) return true;
  // Each end needs a home (a mirrored file or an invocation's splice); the
  // edit then anchors at one end or the other. A task whose body is spelled
  // by a macro is exactly this shape: file signature, expansion-owned end.
  bool ok = true;
  if (begin.isMacroID()) {
    ok = MacroSide(begin) != nullptr;
  } else {
    ok = Mirrored(begin);
  }
  if (end.isMacroID()) {
    ok = MacroSide(end) != nullptr && ok;
  } else {
    ok = Mirrored(end) && ok;
  }
  return ok;
}

void EditSink::ReportSpliceReject(clang::SourceLocation loc,
                                  llvm::StringRef why) {
  ReportCustomDiag(*ctx_, clang::DiagnosticsEngine::Error, loc,
                   "cannot rewrite %0 inside a macro expansion: %1")
      << construct_ << why.str();
}

// Routes one macro-owned edit through its splice; a splice that cannot
// compose it is the residual case and reports precisely here.
template <typename F>
bool EditSink::ViaSplice(clang::SourceLocation begin, clang::SourceLocation end,
                         F&& apply) {
  ExpansionSplice* splice = MacroHome(begin, end);
  if (splice == nullptr) return true;
  std::string why;
  if (apply(splice, &why)) return false;
  ReportSpliceReject(begin, why);
  return true;
}

ExpansionSplice* EditSink::MacroSpliceFor(clang::SourceLocation loc) {
  if (ctx_ == nullptr || !loc.isMacroID()) return nullptr;
  return MacroHome(loc, loc);
}

bool EditSink::DropSpliceTokens(ExpansionSplice* splice, size_t first,
                                size_t last, llvm::StringRef text) {
  std::string why;
  if (splice->DropTokens(first, last, text, &why)) return false;
  ReportSpliceReject(splice->invocation().getBegin(), why);
  return true;
}

bool EditSink::Finish(clang::SourceLocation begin, unsigned length,
                      llvm::StringRef text, bool rejected) {
  if (rejected || ctx_ == nullptr) return rejected;
  const clang::FileID file = rewriter_.getSourceMgr().getFileID(begin);
  if (resnaps_.at(file)(begin, length, text)) return false;
  ReportCustomDiag(*ctx_, clang::DiagnosticsEngine::Error, begin,
                   "cannot maintain line fidelity while rewriting %0")
      << construct_;
  return true;
}

unsigned EditSink::OriginalLength(clang::CharSourceRange range) const {
  clang::SourceManager& sm = rewriter_.getSourceMgr();
  const unsigned begin = sm.getFileOffset(range.getBegin());
  unsigned end = sm.getFileOffset(range.getEnd());
  if (range.isTokenRange()) {
    end += clang::Lexer::MeasureTokenLength(range.getEnd(), sm,
                                            rewriter_.getLangOpts());
  }
  return end - begin;
}

bool EditSink::ReplaceText(clang::SourceRange range, llvm::StringRef text) {
  if (ctx_ == nullptr) return rewriter_.ReplaceText(range, text);
  if (range.getBegin().isMacroID() || range.getEnd().isMacroID()) {
    return ViaSplice(range.getBegin(), range.getEnd(),
                     [&](ExpansionSplice* splice, std::string* why) {
                       return splice->Replace(range.getBegin(), range.getEnd(),
                                              text, why);
                     });
  }
  if (!CanEdit(range.getBegin(), range.getEnd())) return true;
  const unsigned length =
      OriginalLength(clang::CharSourceRange::getTokenRange(range));
  return Finish(range.getBegin(), length, text,
                rewriter_.ReplaceText(range, text));
}

bool EditSink::ReplaceText(clang::SourceLocation start, unsigned length,
                           llvm::StringRef text) {
  if (ctx_ == nullptr) return rewriter_.ReplaceText(start, length, text);
  const clang::SourceLocation end =
      length == 0 ? start : start.getLocWithOffset(length - 1);
  if (start.isMacroID() || end.isMacroID()) {
    return ViaSplice(start, end,
                     [&](ExpansionSplice* splice, std::string* why) {
                       return splice->Replace(start, end, text, why);
                     });
  }
  if (!CanEdit(start, end)) return true;
  return Finish(start, length, text,
                rewriter_.ReplaceText(start, length, text));
}

bool EditSink::InsertTextBefore(clang::SourceLocation loc,
                                llvm::StringRef text) {
  if (ctx_ == nullptr) return rewriter_.InsertTextBefore(loc, text);
  if (loc.isMacroID()) {
    return ViaSplice(loc, loc, [&](ExpansionSplice* splice, std::string* why) {
      return splice->InsertBefore(loc, text, why);
    });
  }
  if (!CanEdit(loc, loc)) return true;
  return Finish(loc, 0, text, rewriter_.InsertTextBefore(loc, text));
}

bool EditSink::InsertTextAfter(clang::SourceLocation loc,
                               llvm::StringRef text) {
  if (ctx_ == nullptr) return rewriter_.InsertTextAfter(loc, text);
  if (loc.isMacroID()) {
    // The Rewriter's InsertTextAfter inserts before the character at the
    // location (ordering same-offset insertions after it); at a token
    // anchor that is the slot before that token.
    return ViaSplice(loc, loc, [&](ExpansionSplice* splice, std::string* why) {
      return splice->InsertBefore(loc, text, why);
    });
  }
  if (!CanEdit(loc, loc)) return true;
  return Finish(loc, 0, text, rewriter_.InsertTextAfter(loc, text));
}

bool EditSink::InsertTextAfterToken(clang::SourceLocation loc,
                                    llvm::StringRef text) {
  if (ctx_ == nullptr) return rewriter_.InsertTextAfterToken(loc, text);
  if (loc.isMacroID()) {
    return ViaSplice(loc, loc, [&](ExpansionSplice* splice, std::string* why) {
      return splice->InsertAfter(loc, text, why);
    });
  }
  if (!CanEdit(loc, loc)) return true;
  const clang::SourceLocation after = clang::Lexer::getLocForEndOfToken(
      loc, 0, rewriter_.getSourceMgr(), rewriter_.getLangOpts());
  const bool rejected = rewriter_.InsertTextAfterToken(loc, text);
  return rejected ? true : Finish(after, 0, text, false);
}

bool EditSink::RemoveText(clang::SourceLocation start, unsigned length) {
  if (ctx_ == nullptr) return rewriter_.RemoveText(start, length);
  const clang::SourceLocation end =
      length == 0 ? start : start.getLocWithOffset(length - 1);
  if (start.isMacroID() || end.isMacroID()) {
    return ViaSplice(start, end,
                     [&](ExpansionSplice* splice, std::string* why) {
                       return splice->Replace(start, end, {}, why);
                     });
  }
  if (!CanEdit(start, end)) return true;
  return Finish(start, length, {}, rewriter_.RemoveText(start, length));
}

bool EditSink::RemoveText(clang::SourceRange range) {
  if (ctx_ == nullptr) return rewriter_.RemoveText(range);
  if (range.getBegin().isMacroID() || range.getEnd().isMacroID()) {
    return ViaSplice(range.getBegin(), range.getEnd(),
                     [&](ExpansionSplice* splice, std::string* why) {
                       return splice->Replace(range.getBegin(), range.getEnd(),
                                              {}, why);
                     });
  }
  if (!CanEdit(range.getBegin(), range.getEnd())) return true;
  const unsigned length =
      OriginalLength(clang::CharSourceRange::getTokenRange(range));
  return Finish(range.getBegin(), length, {}, rewriter_.RemoveText(range));
}

std::string EditSink::getRewrittenText(clang::SourceRange range) const {
  return rewriter_.getRewrittenText(range);
}

std::string EditSink::getRewrittenText(clang::CharSourceRange range) const {
  return rewriter_.getRewrittenText(range);
}

clang::SourceManager& EditSink::getSourceMgr() const {
  return rewriter_.getSourceMgr();
}

const clang::LangOptions& EditSink::getLangOpts() const {
  return rewriter_.getLangOpts();
}

}  // namespace tapa::cc
