#include "edit_sink.h"

#include <utility>

#include "clang/Basic/Diagnostic.h"
#include "clang/Lex/Lexer.h"

#include "frontend/diag.h"

namespace tapa::cc {

EditSink::EditSink(clang::Rewriter& rewriter) : rewriter_(rewriter) {}

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

bool EditSink::CanEdit(clang::SourceLocation begin, clang::SourceLocation end) {
  if (ctx_ == nullptr) return true;
  clang::SourceLocation macro;
  if (begin.isMacroID()) {
    macro = begin;
  } else if (end.isMacroID()) {
    macro = end;
  }
  if (macro.isValid()) {
    ReportCustomDiag(
        *ctx_, clang::DiagnosticsEngine::Error, macro,
        "cannot rewrite %0 inside a macro expansion under "
        "TAPA_ANALYZE_TREE; selective macro expansion is not yet supported")
        << construct_;
    return false;
  }
  if (!begin.isFileID()) return false;
  const clang::FileID file = rewriter_.getSourceMgr().getFileID(begin);
  if (resnaps_.count(file) == 0) {
    ReportCustomDiag(*ctx_, clang::DiagnosticsEngine::Error, begin,
                     "cannot rewrite %0 in non-mirrored file '%1' under "
                     "TAPA_ANALYZE_TREE")
        << construct_ << rewriter_.getSourceMgr().getFilename(begin);
    return false;
  }
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
  if (!CanEdit(start, end)) return true;
  return Finish(start, length, text,
                rewriter_.ReplaceText(start, length, text));
}

bool EditSink::InsertTextBefore(clang::SourceLocation loc,
                                llvm::StringRef text) {
  if (ctx_ == nullptr) return rewriter_.InsertTextBefore(loc, text);
  if (!CanEdit(loc, loc)) return true;
  return Finish(loc, 0, text, rewriter_.InsertTextBefore(loc, text));
}

bool EditSink::InsertTextAfter(clang::SourceLocation loc,
                               llvm::StringRef text) {
  if (ctx_ == nullptr) return rewriter_.InsertTextAfter(loc, text);
  if (!CanEdit(loc, loc)) return true;
  return Finish(loc, 0, text, rewriter_.InsertTextAfter(loc, text));
}

bool EditSink::InsertTextAfterToken(clang::SourceLocation loc,
                                    llvm::StringRef text) {
  if (ctx_ == nullptr) return rewriter_.InsertTextAfterToken(loc, text);
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
  if (!CanEdit(start, end)) return true;
  return Finish(start, length, {}, rewriter_.RemoveText(start, length));
}

bool EditSink::RemoveText(clang::SourceRange range) {
  if (ctx_ == nullptr) return rewriter_.RemoveText(range);
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
