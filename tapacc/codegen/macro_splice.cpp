#include "macro_splice.h"

#include <algorithm>
#include <utility>

#include "clang/Basic/SourceManager.h"
#include "clang/Lex/Preprocessor.h"

namespace tapa::cc {

// ── Recording ─────────────────────────────────────────────────────────────

TokenRecorder::TokenRecorder(clang::Preprocessor& pp)
    : collector_(std::make_unique<clang::syntax::TokenCollector>(pp)) {}

const clang::syntax::TokenBuffer& TokenRecorder::Consume() {
  if (buffer_ == nullptr) {
    buffer_ = std::make_unique<clang::syntax::TokenBuffer>(
        std::move(*collector_).consume());
    collector_.reset();
  }
  return *buffer_;
}

// ── The owning invocation ─────────────────────────────────────────────────

clang::CharSourceRange OutermostInvocation(const clang::SourceManager& sm,
                                           clang::SourceLocation loc) {
  clang::CharSourceRange range = sm.getImmediateExpansionRange(loc);
  while (range.getBegin().isMacroID()) {
    range = sm.getImmediateExpansionRange(range.getBegin());
  }
  return range;
}

// The expanded tokens of one outermost invocation, straight from the token
// buffer's own mapping: it records only outermost expansions, so macros used
// inside a macro body or in an argument fold into the outer one -- exactly
// the semantics §3.5 asks for.
static llvm::ArrayRef<clang::syntax::Token> ExpansionOf(
    const clang::syntax::TokenBuffer& tokens, const clang::SourceManager& sm,
    const clang::CharSourceRange& invocation) {
  const clang::syntax::Token* first =
      tokens.spelledTokenContaining(sm.getExpansionLoc(invocation.getBegin()));
  if (first == nullptr) return {};
  const std::optional<clang::syntax::TokenBuffer::Expansion> expansion =
      tokens.expansionStartingAt(first);
  if (!expansion) return {};
  return expansion->Expanded;
}

// ── One invocation's splice ───────────────────────────────────────────────

ExpansionSplice::ExpansionSplice(const clang::SourceManager& sm,
                                 clang::CharSourceRange invocation,
                                 llvm::ArrayRef<clang::syntax::Token> tokens)
    : sm_(sm),
      invocation_(invocation),
      tokens_(tokens),
      slots_(tokens.size() + 1) {}

// First expanded token at or after `loc`, in translation-unit order. An
// anchor before the first token or past the last one is outside the
// invocation: there is no honest slot for it.
std::optional<size_t> ExpansionSplice::IndexAtOrAfter(
    clang::SourceLocation loc) const {
  for (size_t i = 0; i < tokens_.size(); ++i) {
    if (tokens_[i].location() == loc) return i;
  }
  if (tokens_.empty() ||
      sm_.isBeforeInTranslationUnit(loc, tokens_.front().location())) {
    return std::nullopt;
  }
  const auto* it = std::lower_bound(
      tokens_.begin(), tokens_.end(), loc,
      [&](const clang::syntax::Token& t, clang::SourceLocation l) {
        return sm_.isBeforeInTranslationUnit(t.location(), l);
      });
  if (it == tokens_.end()) return std::nullopt;
  return static_cast<size_t>(it - tokens_.begin());
}

// Last expanded token at or before `loc`. Anchors past the last token's
// start are outside the invocation: no edit lands after the expansion's
// last token by mapping alone (an InsertAfter the invocation's end is a
// file-level edit at the spelled range, not a splice edit).
std::optional<size_t> ExpansionSplice::IndexAtOrBefore(
    clang::SourceLocation loc) const {
  const std::optional<size_t> after = IndexAtOrAfter(loc);
  if (!after) return std::nullopt;
  if (tokens_[*after].location() == loc) return *after;
  if (*after == 0) return std::nullopt;
  return *after - 1;
}

llvm::StringRef ExpansionSplice::TokenText(size_t index) const {
  return tokens_[index].text(sm_);
}

bool ExpansionSplice::InsertBefore(clang::SourceLocation loc,
                                   llvm::StringRef text, std::string* why) {
  const std::optional<size_t> at = IndexAtOrAfter(loc);
  if (!at) {
    *why = "the edit anchors outside the invocation's recorded tokens";
    return false;
  }
  Append(*at, text);
  return true;
}

bool ExpansionSplice::InsertAfter(clang::SourceLocation loc,
                                  llvm::StringRef text, std::string* why) {
  const std::optional<size_t> at = IndexAtOrBefore(loc);
  if (!at) {
    *why = "the edit anchors outside the invocation's recorded tokens";
    return false;
  }
  Append(*at + 1, text);
  return true;
}

bool ExpansionSplice::Replace(clang::SourceLocation begin,
                              clang::SourceLocation end, llvm::StringRef text,
                              std::string* why) {
  const std::optional<size_t> first = IndexAtOrAfter(begin);
  const std::optional<size_t> last = IndexAtOrBefore(end);
  if (!first || !last || *first > *last) {
    *why = "the edit anchors outside the invocation's recorded tokens";
    return false;
  }
  return DropTokens(*first, *last, text, why);
}

bool ExpansionSplice::DropTokens(size_t first, size_t last,
                                 llvm::StringRef text, std::string* why) {
  if (first > last || last >= tokens_.size()) {
    *why = "the edit anchors outside the invocation's recorded tokens";
    return false;
  }
  // Two replacements claiming one token have no single honest rendering:
  // compose-order would silently decide which text survives.
  for (size_t i = first; i <= last; ++i) {
    if (dropped_.count(i) != 0) {
      *why = "it overlaps another replacement of the same invocation";
      return false;
    }
  }
  for (size_t i = first; i <= last; ++i) dropped_.insert(i);
  Append(first, text);
  edited_ = true;
  return true;
}

void ExpansionSplice::Append(size_t slot, llvm::StringRef text) {
  std::string& piece = slots_[slot];
  if (!piece.empty() && piece.back() != '\n') piece += ' ';
  piece += text.str();
  edited_ = true;
}

std::string ExpansionSplice::Render() const {
  std::string out;
  const auto append_piece = [&out](const std::string& piece) {
    if (piece.empty()) return;
    out += piece;
    if (piece.back() != ' ' && piece.back() != '\n') out += ' ';
  };
  for (size_t i = 0; i < tokens_.size(); ++i) {
    append_piece(slots_[i]);
    if (dropped_.count(i) != 0) continue;
    out += tokens_[i].text(sm_).str();
    out += ' ';
  }
  append_piece(slots_[tokens_.size()]);
  while (!out.empty() && out.back() == ' ') out.pop_back();
  return out;
}

// ── The TU's splices ──────────────────────────────────────────────────────

MacroSplices::MacroSplices(const clang::syntax::TokenBuffer& tokens,
                           const clang::SourceManager& sm)
    : tokens_(tokens), sm_(sm) {}

ExpansionSplice* MacroSplices::SpliceFor(clang::SourceLocation loc,
                                         std::string* why) {
  const clang::CharSourceRange invocation = OutermostInvocation(sm_, loc);
  const clang::SourceLocation key = invocation.getBegin();
  const auto it = splices_.find(key);
  if (it != splices_.end()) return it->second.get();
  auto splice = std::make_unique<ExpansionSplice>(
      sm_, invocation, ExpansionOf(tokens_, sm_, invocation));
  if (splice->size() == 0) {
    *why = "the token stream records no expansion for its invocation";
    return nullptr;
  }
  ExpansionSplice* const raw = splice.get();
  splices_.emplace(key, std::move(splice));
  return raw;
}

std::vector<ExpansionSplice*> MacroSplices::Edited() {
  std::vector<ExpansionSplice*> edited;
  for (auto& [key, splice] : splices_) {
    (void)key;
    if (splice->has_edits()) edited.push_back(splice.get());
  }
  return edited;
}

}  // namespace tapa::cc
