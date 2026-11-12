#include "tree_writer.h"

#include <algorithm>
#include <array>
#include <cassert>
#include <set>
#include <utility>

#include "clang/AST/ASTContext.h"
#include "clang/Basic/LangOptions.h"
#include "clang/Basic/SourceManager.h"
#include "clang/Lex/Lexer.h"
#include "llvm/ADT/STLExtras.h"
#include "llvm/ADT/SmallString.h"
#include "llvm/ADT/StringRef.h"
#include "llvm/Support/FileSystem.h"
#include "llvm/Support/MemoryBuffer.h"
#include "llvm/Support/Path.h"
#include "llvm/Support/SHA256.h"
#include "llvm/Support/raw_ostream.h"

namespace tapa::cc {

std::string CanonicalPath(llvm::StringRef name) {
  llvm::SmallString<256> real;
  if (!llvm::sys::fs::real_path(name, real)) return real.str().str();
  return name.str();
}

// ── Include recording ─────────────────────────────────────────────────────

IncludeRecorder::IncludeRecorder(clang::SourceManager& sm,
                                 std::vector<IncludeDirective>* log)
    : sm_(sm), log_(log) {}

void IncludeRecorder::InclusionDirective(
    clang::SourceLocation hash_loc, const clang::Token&,
    llvm::StringRef file_name, bool is_angled,
    clang::CharSourceRange filename_range, clang::OptionalFileEntryRef file,
    llvm::StringRef, llvm::StringRef, const clang::Module*, bool,
    clang::SrcMgr::CharacteristicKind file_type) {
  clang::OptionalFileEntryRef writing =
      sm_.getFileEntryRefForID(sm_.getFileID(hash_loc));
  if (!writing) return;  // built-in virtual buffers carry no file entry

  IncludeDirective record;
  record.writing_file = CanonicalPath(writing->getName());
  record.offset = sm_.getFileOffset(hash_loc);
  record.spelling = file_name.str();
  record.angled = is_angled;
  // The characteristic kind is the include lookup's own verdict on the
  // search entry used: C_User for -I and includer-relative quoted
  // lookups, C_System/C_ExternCSystem for -isystem-style entries.
  record.from_system_search = file_type == clang::SrcMgr::C_System ||
                              file_type == clang::SrcMgr::C_ExternCSystem;
  record.filename_range = filename_range;
  if (file) {
    record.resolved = CanonicalPath(file->getName());
    record.resolved_entry = file;
  }
  // A file entered again sights its directives again: keep the first
  // sighting of each site. Two directives at different sites are two
  // records, each with its own spelling to rewrite.
  for (const IncludeDirective& seen : *log_) {
    if (seen.writing_file == record.writing_file &&
        seen.offset == record.offset && seen.spelling == record.spelling &&
        seen.angled == record.angled && seen.resolved == record.resolved) {
      return;
    }
  }
  log_->push_back(std::move(record));
}

std::map<std::string, clang::FileID> MirrorFiles(
    clang::ASTContext& ctx, const std::vector<std::string>& main_files,
    const std::vector<IncludeDirective>& log) {
  clang::SourceManager& sm = ctx.getSourceManager();
  // Both frontend passes use this predicate so indexing and tree
  // materialization agree about whether a source file is user code.
  std::set<std::string> mirrored(main_files.begin(), main_files.end());
  for (const IncludeDirective& record : log) {
    if (!record.resolved.empty() && !record.from_system_search) {
      mirrored.insert(record.resolved);
    }
  }
  std::map<std::string, clang::FileID> files;
  auto remember = [&](const std::string& path, clang::FileID file) {
    files.try_emplace(path, file);
  };
  if (const clang::OptionalFileEntryRef main =
          sm.getFileEntryRefForID(sm.getMainFileID())) {
    remember(CanonicalPath(main->getName()), sm.getMainFileID());
  }
  for (const IncludeDirective& record : log) {
    if (mirrored.count(record.writing_file) > 0) {
      remember(record.writing_file,
               sm.getFileID(record.filename_range.getBegin()));
    }
    if (mirrored.count(record.resolved) > 0 && record.resolved_entry) {
      remember(record.resolved, sm.getOrCreateFileID(*record.resolved_entry,
                                                     clang::SrcMgr::C_User));
    }
  }
  return files;
}

// ── Mirror layout ─────────────────────────────────────────────────────────

std::string TreeLayout::SrcRoot(const std::vector<std::string>& main_files) {
  assert(!main_files.empty());
  // The root is a directory, so the file names never participate: reduce
  // every input to its parent's components and keep the shared prefix.
  // Canonical absolute paths have no "." or ".." components, so a plain
  // split matches llvm::sys::path::components without its iterator dance.
  std::vector<std::vector<std::string>> dirs;
  dirs.reserve(main_files.size());
  for (const std::string& file : main_files) {
    std::vector<std::string> parts;
    llvm::StringRef parent =
        llvm::sys::path::parent_path(file, llvm::sys::path::Style::native);
    if (parent.starts_with("/")) parts.emplace_back("/");
    llvm::SmallVector<llvm::StringRef, 16> pieces;
    parent.split(pieces, '/');
    for (const llvm::StringRef piece : pieces) {
      if (!piece.empty() && piece != ".") parts.emplace_back(piece.str());
    }
    dirs.push_back(std::move(parts));
  }
  size_t count = dirs.front().size();
  for (const std::vector<std::string>& parts : dirs) {
    count = std::min(count, parts.size());
  }
  size_t shared = 0;
  while (shared < count) {
    bool equal = true;
    for (const std::vector<std::string>& parts : dirs) {
      equal = equal && parts[shared] == dirs.front()[shared];
    }
    if (!equal) break;
    ++shared;
  }
  llvm::SmallString<256> root;
  for (size_t i = 0; i < shared; ++i) {
    if (dirs.front()[i] == "/") {
      root = "/";
    } else {
      llvm::sys::path::append(root, dirs.front()[i]);
    }
  }
  return root.str().str();
}

TreeLayout::TreeLayout(std::string src_root) : src_root_(std::move(src_root)) {
  // SrcRoot never leaves a trailing separator, but stay robust to one so
  // InRoot's boundary check keeps meaning component boundaries.
  while (src_root_.size() > 1 && src_root_.back() == '/') src_root_.pop_back();
}

bool TreeLayout::InRoot(llvm::StringRef abs_path) const {
  // A raw string prefix would swallow "/ab" under root "/a"
  // (replace_path_prefix does exactly that), so test the boundary here.
  if (!abs_path.starts_with(src_root_)) return false;
  if (src_root_ == "/") return true;
  return abs_path.size() > src_root_.size() &&
         abs_path[src_root_.size()] == '/';
}

std::string TreeLayout::InRootKey(llvm::StringRef abs_path) const {
  llvm::StringRef key = abs_path.substr(src_root_.size());
  return key.starts_with("/") ? key.substr(1).str() : key.str();
}

std::string TreeLayout::ExternalKey(llvm::StringRef abs_path) {
  llvm::SHA256 sha;
  sha.update(abs_path);
  const std::array<uint8_t, 32> digest = sha.final();
  const std::string hex = llvm::toHex(digest, /*LowerCase=*/true);
  return (llvm::Twine("_external/") + llvm::StringRef(hex).substr(0, 8) + "/" +
          llvm::sys::path::filename(abs_path, llvm::sys::path::Style::native))
      .str();
}

std::string TreeLayout::TreeKey(llvm::StringRef abs_path) const {
  return InRoot(abs_path) ? InRootKey(abs_path) : ExternalKey(abs_path);
}

bool TreeLayout::ResolvesAsWritten(const IncludeDirective& record) const {
  const auto joined = [](llvm::StringRef dir, llvm::StringRef spelling) {
    llvm::SmallString<256> path(dir);
    llvm::sys::path::append(path, spelling);
    llvm::sys::path::remove_dots(path, /*remove_dot_dot=*/true);
    return path.str().str();
  };
  if (!InRoot(record.resolved)) {
    // A quoted spelling always moves to the bucket's key; an angled one
    // resolves through the bucket directory the manifest lists, which
    // holds the file under its bare name.
    return record.angled &&
           record.spelling == llvm::sys::path::filename(record.resolved);
  }
  // In the root, both files keep their relative position, and the tree
  // root itself is on the include path; a directory the user's own `-I`
  // reached is neither.
  if (!record.angled && InRoot(record.writing_file) &&
      joined(llvm::sys::path::parent_path(record.writing_file),
             record.spelling) == record.resolved) {
    return true;
  }
  return joined(src_root_, record.spelling) == record.resolved;
}

bool TreeLayout::Register(const std::string& abs_path, std::string* key,
                          std::string* error) {
  const std::string assigned = TreeKey(abs_path);
  const auto [owned, inserted] = key_to_path_.emplace(assigned, abs_path);
  if (!inserted && owned->second != abs_path) {
    *error = "two distinct files map to the same rewritten-tree path '" +
             assigned + "': '" + owned->second + "' and '" + abs_path + "'";
    return false;
  }
  path_to_key_[abs_path] = assigned;
  *key = assigned;
  return true;
}

const std::string* TreeLayout::KeyOf(llvm::StringRef abs_path) const {
  const auto it = path_to_key_.find(abs_path.str());
  return it == path_to_key_.end() ? nullptr : &it->second;
}

// ── Per-file buffer with #line re-snap ────────────────────────────────────

namespace {

// Writes the identity of original text at `loc`. In an ordinary file the
// mirror keeps using its canonical physical path. If a user #line directive
// remaps either component, the presumed identity is authoritative so the
// generated marker preserves both __FILE__ and __LINE__.
void WriteOriginalLineIdentity(llvm::raw_ostream& os,
                               const clang::SourceManager& sm,
                               clang::SourceLocation loc,
                               const std::string& physical_path) {
  const clang::PresumedLoc presumed = sm.getPresumedLoc(loc);
  const clang::PresumedLoc physical =
      sm.getPresumedLoc(loc, /*UseLineDirectives=*/false);
  const bool remapped =
      presumed.isValid() &&
      (!physical.isValid() || presumed.getLine() != physical.getLine() ||
       llvm::StringRef(presumed.getFilename()) != physical.getFilename());
  if (remapped) {
    os << presumed.getLine() << " \"" << presumed.getFilename() << "\"";
    return;
  }
  os << (presumed.isValid() ? presumed.getLine()
                            : sm.getSpellingLineNumber(loc))
     << " \"" << physical_path << "\"";
}

}  // namespace

TreeFileBuffer::TreeFileBuffer(clang::SourceManager& sm,
                               clang::Rewriter& rewriter, clang::FileID file,
                               std::string abs_path)
    : sm_(sm),
      rewriter_(rewriter),
      file_(file),
      abs_path_(std::move(abs_path)) {}

bool TreeFileBuffer::ReplaceWithLineResnap(clang::CharSourceRange range,
                                           llvm::StringRef text) {
  if (!range.getBegin().isFileID() || !range.getEnd().isFileID()) return false;
  const unsigned begin = sm_.getFileOffset(range.getBegin());
  unsigned end = sm_.getFileOffset(range.getEnd());
  if (range.isTokenRange()) {
    // The preprocessor hands out char ranges, but stay correct for token
    // ranges: extend to the end of the last token, as the Rewriter does.
    end += clang::Lexer::MeasureTokenLength(range.getEnd(), sm_,
                                            rewriter_.getLangOpts());
  }
  return ApplyEdit(range.getBegin(), end - begin, text);
}

bool TreeFileBuffer::InsertGuardOpening(clang::SourceRange range,
                                        llvm::StringRef opening) {
  return range.getBegin().isFileID() && ApplyEdit(range.getBegin(), 0, opening);
}

bool TreeFileBuffer::InsertGuardClosing(clang::SourceRange range,
                                        llvm::StringRef closing) {
  const clang::SourceLocation after_last_token =
      clang::Lexer::getLocForEndOfToken(range.getEnd(), 0, sm_,
                                        rewriter_.getLangOpts());
  return after_last_token.isFileID() && ApplyEdit(after_last_token, 0, closing);
}

std::string TreeFileBuffer::SyntheticLead(clang::SourceLocation begin,
                                          llvm::StringRef text) const {
  if (!begin.isFileID() || text.empty() || !text.contains('\n')) {
    return text.str();
  }
  // The leading newline keeps the marker on its own line whatever precedes
  // the anchor; the trailing one ends it. The anchor's presumed line names
  // the same user construct even when a #line directive remapped it.
  const clang::PresumedLoc presumed = sm_.getPresumedLoc(begin);
  const unsigned line = presumed.isValid() ? presumed.getLine()
                                           : sm_.getSpellingLineNumber(begin);
  return "\n#line " + std::to_string(line) + " \"tapa:" + abs_path_ + "\"\n" +
         text.str();
}

bool TreeFileBuffer::ApplyEdit(clang::SourceLocation begin, unsigned length,
                               llvm::StringRef text) {
  if (!begin.isFileID() || sm_.getFileID(begin) != file_) return false;
  const std::string composed =
      length == 0 ? SyntheticLead(begin, text) : text.str();
  // Insertions go through InsertTextAfter, not ReplaceText(len 0): a
  // replace records its delta at an offset the marker's own mapping then
  // excludes, which would place the marker before the inserted text.
  const bool rejected = length == 0
                            ? rewriter_.InsertTextAfter(begin, composed)
                            : rewriter_.ReplaceText(begin, length, composed);
  return !rejected && ResnapAfterEdit(begin, length, composed);
}

bool TreeFileBuffer::ResnapAfterEdit(clang::SourceLocation begin,
                                     unsigned length, llvm::StringRef text) {
  if (!begin.isFileID() || sm_.getFileID(begin) != file_) return false;
  const llvm::StringRef original = sm_.getBufferData(file_);
  const unsigned begin_offset = sm_.getFileOffset(begin);
  const unsigned end_offset = begin_offset + length;
  const unsigned old_lines =
      llvm::count(original.substr(begin_offset, length), '\n');
  had_edits_ = true;
  if (old_lines == llvm::count(text, '\n')) return true;  // count preserved

  // The marker must start its own line and pin the line number of the
  // original character that follows the edit, in original coordinates
  // (every line-count change re-snaps immediately, so original numbering
  // stays the truth no matter what earlier edits did physically).
  const llvm::StringRef rest = original.substr(
      std::min(end_offset, static_cast<unsigned>(original.size())));
  if (rest.empty()) return true;  // nothing follows the edit
  unsigned marker_offset = end_offset;
  if (rest.front() == '\n') {
    // The edit ends exactly at a line boundary: pin the NEXT line from its
    // own start (the unchanged newline already terminates the last written
    // line).
    if (rest.size() == 1) return true;  // only the final newline follows
    marker_offset = end_offset + 1;
  }
  const clang::SourceLocation at =
      sm_.getLocForStartOfFile(file_).getLocWithOffset(marker_offset);
  std::string marker;
  llvm::raw_string_ostream os(marker);
  // The marker always starts its own line, unconditionally: another edit
  // recorded at the same offset (a helper's inline attribute, a guard edge)
  // can be emitted ahead of it, and a `#line` directive glued to prior
  // text is a syntax error. A leading blank line is legal and harmless;
  // pins are absolute.
  os << (begin_offset > 0 ? "\n" : "") << "#line ";
  WriteOriginalLineIdentity(os, sm_, at, abs_path_);
  os << "\n";
  // InsertTextAfter, not Before: an edit at the same offset (a pure
  // insertion's own marker) must land after the inserted text, and
  // RewriteBuffer maps equal offsets through that flag.
  rewriter_.InsertTextAfter(at, marker);
  return true;
}

std::string TreeFileBuffer::Render() {
  // Untouched files are byte-identical copies of the original. An edited
  // file opens by adopting the original physical identity (`#line 1
  // "<abs>"`), so diagnostics and `__FILE__` in its untouched head cite the
  // user's source, not the mirror. This initial adoption deliberately
  // precedes any user #line directive; re-snap markers use presumed identity
  // for the original text after later edits.
  if (!had_edits_) return sm_.getBufferData(file_).str();
  std::string out = "#line 1 \"" + abs_path_ + "\"\n";
  llvm::raw_string_ostream os(out);
  rewriter_.getEditBuffer(file_).write(os);
  return out;
}

// ── One-TU editing session ────────────────────────────────────────────────

TreeSession::TreeSession(clang::ASTContext& ctx,
                         const std::vector<IncludeDirective>& log,
                         const std::vector<std::string>& main_files,
                         const clang::syntax::TokenBuffer* tokens,
                         std::vector<ExpansionPragma> pragmas)
    : rewriter_(ctx.getSourceManager(), ctx.getLangOpts()),
      edits_(ctx, rewriter_),
      tokens_(tokens),
      pragmas_(std::move(pragmas)) {
  clang::SourceManager& sm = ctx.getSourceManager();
  if (tokens != nullptr) {
    splices_.emplace(*tokens, sm, pragmas_);
    edits_.RouteMacroEdits(&*splices_);
  }
  // Insertions routed through EditSink compose with the same synthetic
  // identity TreeFileBuffer::ApplyEdit applies to its own.
  edits_.ComposeInsertions([this](clang::SourceLocation loc,
                                  llvm::StringRef text) {
    const TreeFileBuffer* const buffer = BufferForLocation(loc);
    return buffer == nullptr ? text.str() : buffer->SyntheticLead(loc, text);
  });
  for (const auto& [path, file] : MirrorFiles(ctx, main_files, log)) {
    auto buffer = std::make_unique<TreeFileBuffer>(sm, rewriter_, file, path);
    TreeFileBuffer* const ptr = buffer.get();
    edits_.TrackFile(file, [ptr](clang::SourceLocation begin, unsigned length,
                                 llvm::StringRef text) {
      return ptr->ResnapAfterEdit(begin, length, text);
    });
    buffers_by_file_[file] = ptr;
    buffers_.emplace(path, std::move(buffer));
  }
}

void TreeSession::ScratchSplices(std::optional<MacroSplices>& splices) const {
  if (tokens_ != nullptr) {
    splices.emplace(*tokens_, rewriter_.getSourceMgr(), pragmas_);
  }
}

TreeFileBuffer* TreeSession::BufferForPath(llvm::StringRef path) {
  const auto it = buffers_.find(path.str());
  return it == buffers_.end() ? nullptr : it->second.get();
}

TreeFileBuffer* TreeSession::BufferForLocation(clang::SourceLocation loc) {
  if (!loc.isFileID()) return nullptr;
  const auto it = buffers_by_file_.find(edits_.getSourceMgr().getFileID(loc));
  return it == buffers_by_file_.end() ? nullptr : it->second;
}

// The opening renders immediately before the definition and the closing
// immediately after it. A macro-owned anchor (a body spelled by a macro:
// the closing brace, or the whole definition) composes into that
// invocation's splice, at the slot before its first / after its last
// token -- exactly where the definition renders.
bool TreeSession::InsertGuardOpening(clang::SourceRange range,
                                     llvm::StringRef opening,
                                     std::string construct) {
  edits_.Describe(std::move(construct));
  if (range.getBegin().isMacroID())
    return !edits_.InsertTextBefore(range.getBegin(), opening);
  if (!edits_.CanRewrite(range)) return false;
  TreeFileBuffer* const buffer = BufferForLocation(range.getBegin());
  return buffer != nullptr && buffer->InsertGuardOpening(range, opening);
}

bool TreeSession::InsertGuardClosing(clang::SourceRange range,
                                     llvm::StringRef closing,
                                     std::string construct) {
  edits_.Describe(std::move(construct));
  if (range.getEnd().isMacroID())
    return !edits_.InsertTextAfterToken(range.getEnd(), closing);
  if (!edits_.CanRewrite(range)) return false;
  TreeFileBuffer* const buffer = BufferForLocation(range.getBegin());
  return buffer != nullptr && buffer->InsertGuardClosing(range, closing);
}

void TreeSession::FlushMacroSplices() {
  if (!splices_.has_value()) return;
  for (ExpansionSplice* splice : splices_->Edited()) {
    TreeFileBuffer* const buffer =
        BufferForLocation(splice->invocation().getBegin());
    // An unmirrored spelling file was rejected when its edits arrived.
    if (buffer != nullptr) {
      buffer->ReplaceWithLineResnap(splice->invocation(), splice->Render());
    }
  }
}

// ── The writer ────────────────────────────────────────────────────────────

TreeWriter::TreeWriter(std::string out_root,
                       std::vector<std::string> main_files)
    : out_root_(std::move(out_root)),
      main_files_(std::move(main_files)),
      layout_(TreeLayout::SrcRoot(main_files_)) {}

bool TreeWriter::AddTu(clang::ASTContext& ctx,
                       std::vector<IncludeDirective> log, std::string* error) {
  TreeSession session(ctx, log, main_files_, /*tokens=*/nullptr);
  return AddTu(ctx, std::move(log), session, error);
}

bool TreeWriter::AddTu(clang::ASTContext& ctx,
                       std::vector<IncludeDirective> log, TreeSession& session,
                       std::string* error) {
  // Include-line rewriting: an include of a mirrored file whose spelling
  // the mirror does not resolve as written moves to the file's tree key
  // (the tree root is on the include path, so it resolves), keeping its
  // quotes or brackets: every quoted include of an out-of-root mirror, and
  // any include that reached its target through a user `-I` directory. The
  // rest keep what the user wrote. These edits share the same per-file
  // buffers as the caller's decl rewrites.
  for (const IncludeDirective& record : log) {
    if (record.resolved.empty() || record.from_system_search ||
        layout_.ResolvesAsWritten(record)) {
      continue;
    }
    TreeFileBuffer* const buffer = session.BufferForPath(record.writing_file);
    if (buffer == nullptr) continue;
    const std::string key = layout_.TreeKey(record.resolved);
    session.edits().Describe("include directive");
    if (!session.edits().CanRewrite(record.filename_range) ||
        !buffer->ReplaceWithLineResnap(
            record.filename_range,
            record.angled ? "<" + key + ">" : "\"" + key + "\"")) {
      *error = "cannot rewrite the include of '" + record.resolved + "' in '" +
               record.writing_file + "'";
      return false;
    }
  }

  // The session covers every mirrored file this TU can speak for. First
  // sighting owns a file; an unedited buffer renders its original bytes.
  // Every later sighting must compute the same bytes: one mirror serves
  // every task variant, so a header rewritten differently per TU (which
  // can only happen through context-dependent rewrites, e.g. a macro that
  // expands differently per TU) has no single honest rendering.
  const clang::SourceManager& sm = ctx.getSourceManager();
  const std::string tu = CanonicalPath(
      sm.getFilename(sm.getLocForStartOfFile(sm.getMainFileID())));
  for (const auto& [path, buffer] : session.buffers()) {
    const std::string bytes = buffer->Render();
    const auto [it, inserted] = rendered_.try_emplace(path, bytes);
    if (inserted) {
      owners_[path] = tu;
      continue;
    }
    if (it->second != bytes) {
      *error = "mirrored file '" + path +
               "' is rewritten differently by translation units '" +
               owners_[path] + "' and '" + tu +
               "'; a file shared across translation units must rewrite "
               "identically in each (divergence can only come from "
               "context-dependent rewrites such as a macro expanding "
               "differently per translation unit)";
      return false;
    }
  }
  return true;
}

std::vector<std::string> TreeWriter::MainFileKeys() const {
  std::vector<std::string> keys;
  keys.reserve(main_files_.size());
  for (const std::string& path : main_files_) {
    keys.push_back(layout_.InRoot(path) ? layout_.InRootKey(path)
                                        : TreeLayout::ExternalKey(path));
  }
  return keys;
}

std::vector<std::string> TreeWriter::ExternalBuckets() const {
  std::set<std::string> buckets;
  for (const auto& [path, bytes] : rendered_) {
    (void)bytes;
    if (layout_.InRoot(path)) continue;
    buckets.insert(llvm::sys::path::parent_path(TreeLayout::ExternalKey(path),
                                                llvm::sys::path::Style::native)
                       .str());
  }
  return {buckets.begin(), buckets.end()};
}

constexpr char kOwnershipManifestName[] = ".tapa-tree-manifest";
constexpr char kOwnershipManifestHeader[] = "tapa-tree-ownership-v1\n";

namespace {

// Reads the files TAPA owned on the previous run. An absent manifest means
// that this directory has no TAPA-owned files yet; an unrecognized existing
// manifest is a collision, not permission to adopt the directory.
bool ReadOwnershipManifest(const std::string& path,
                           std::set<std::string>* owned, std::string* error) {
  llvm::sys::fs::file_status status;
  std::error_code ec = llvm::sys::fs::status(path, status, /*follow=*/false);
  if (ec == std::errc::no_such_file_or_directory) return true;
  if (ec) {
    *error =
        "cannot inspect ownership manifest '" + path + "': " + ec.message();
    return false;
  }
  if (status.type() != llvm::sys::fs::file_type::regular_file) {
    *error = "ownership manifest '" + path + "' is not a regular file";
    return false;
  }
  const auto buffer = llvm::MemoryBuffer::getFile(path);
  if (!buffer) {
    *error = "cannot read ownership manifest '" + path +
             "': " + buffer.getError().message();
    return false;
  }
  llvm::StringRef rest = (*buffer)->getBuffer();
  if (!rest.consume_front(kOwnershipManifestHeader)) {
    *error = "ownership manifest '" + path + "' has an unrecognized format";
    return false;
  }
  while (!rest.empty()) {
    const size_t end = rest.find('\n');
    if (end == llvm::StringRef::npos || end == 0) {
      *error = "ownership manifest '" + path + "' is malformed";
      return false;
    }
    owned->insert(rest.substr(0, end).str());
    rest = rest.substr(end + 1);
  }
  return true;
}

bool HasOwnedPrefix(const std::set<std::string>& owned,
                    const std::string& prefix) {
  const auto it = owned.lower_bound(prefix);
  return it != owned.end() && llvm::StringRef(*it).starts_with(prefix);
}

// Prunes only stale files recorded in TAPA's previous ownership manifest.
// Unlisted files and all directories are left alone: -emit-dir may be an
// arbitrary existing directory. Symlinks are never followed; a previously
// owned link is just a file entry that may be removed.
bool PruneDirectory(const std::string& dir, const std::string& prefix,
                    const std::set<std::string>& owned,
                    const std::set<std::string>& keys, std::string* error) {
  if (owned.empty()) return true;
  std::error_code ec;
  llvm::sys::fs::directory_iterator it(dir, ec);
  if (ec) {
    if (ec == std::errc::no_such_file_or_directory) return true;
    *error = "cannot scan '" + dir + "': " + ec.message();
    return false;
  }
  for (; it != llvm::sys::fs::directory_iterator(); it = it.increment(ec)) {
    if (ec) break;
    const std::string path = it->path();
    const std::string name = llvm::sys::path::filename(path).str();
    llvm::sys::fs::file_status status;
    if ((ec = llvm::sys::fs::status(path, status, /*follow=*/false))) break;
    if (status.type() == llvm::sys::fs::file_type::directory_file) {
      const std::string child_prefix = prefix + name + "/";
      if (HasOwnedPrefix(owned, child_prefix) &&
          !PruneDirectory(path, child_prefix, owned, keys, error)) {
        return false;
      }
    } else if (owned.count(prefix + name) > 0 &&
               keys.count(prefix + name) == 0) {
      ec = llvm::sys::fs::remove(path);
    }
    if (ec) break;
  }
  if (ec) {
    *error = "cannot prune '" + dir + "': " + ec.message();
    return false;
  }
  return true;
}

}  // namespace

bool TreeWriter::Write(std::string* error) {
  std::set<std::string> keys;
  for (const auto& [path, bytes] : rendered_) {
    (void)bytes;  // registration is about the paths
    std::string key;
    if (!layout_.Register(path, &key, error)) return false;
    keys.insert(std::move(key));
  }
  llvm::SmallString<256> manifest_path(out_root_);
  llvm::sys::path::append(manifest_path, kOwnershipManifestName);
  const std::string manifest = manifest_path.str().str();
  std::set<std::string> owned;
  if (!ReadOwnershipManifest(manifest, &owned, error)) return false;

  const std::string manifest_prefix = std::string(kOwnershipManifestName) + "/";
  for (const std::string& key : keys) {
    if (key == kOwnershipManifestName ||
        llvm::StringRef(key).starts_with(manifest_prefix)) {
      *error = "rewritten-tree path '" + key +
               "' collides with TAPA's ownership manifest";
      return false;
    }
  }

  // Check every destination before pruning or writing anything. In
  // particular, equal bytes do not make an existing unowned file ours.
  for (const auto& [path, content] : rendered_) {
    (void)content;
    const std::string key = *layout_.KeyOf(path);
    llvm::SmallString<256> out(out_root_);
    llvm::sys::path::append(out, key);
    llvm::sys::fs::file_status status;
    const std::error_code ec =
        llvm::sys::fs::status(out, status, /*follow=*/false);
    if (ec == std::errc::no_such_file_or_directory ||
        ec == std::errc::not_a_directory) {
      continue;
    }
    if (ec) {
      *error = "cannot inspect '" + out.str().str() + "': " + ec.message();
      return false;
    }
    if (!llvm::sys::fs::exists(status)) continue;
    if (owned.count(key) == 0) {
      *error = "refusing to overwrite unowned path '" + out.str().str() + "'";
      return false;
    }
    if (status.type() != llvm::sys::fs::file_type::regular_file) {
      *error = "cannot write '" + out.str().str() +
               "': the owned path is not a regular file";
      return false;
    }
  }

  // The mirror is a pure function of this run's inputs, but only files TAPA
  // itself wrote on an earlier run are eligible for stale pruning.
  if (!PruneDirectory(out_root_, "", owned, keys, error)) return false;

  // Materialize in a fixed order so identical inputs visit the same paths
  // the same way every run; unchanged bytes are skipped.
  for (const auto& [path, content] : rendered_) {
    const std::string key = *layout_.KeyOf(path);
    llvm::SmallString<256> out(out_root_);
    llvm::sys::path::append(out, key);
    if (const auto existing = llvm::MemoryBuffer::getFile(out)) {
      if ((*existing)->getBuffer() == content) continue;
    }
    llvm::SmallString<256> parent(llvm::sys::path::parent_path(out));
    if (std::error_code ec = llvm::sys::fs::create_directories(parent)) {
      *error = "cannot create directory '" + parent.str().str() +
               "': " + ec.message();
      return false;
    }
    std::error_code ec;
    llvm::raw_fd_ostream stream(out, ec);
    if (ec) {
      *error = "cannot write '" + out.str().str() + "': " + ec.message();
      return false;
    }
    stream << content;
  }

  if (std::error_code ec = llvm::sys::fs::create_directories(out_root_)) {
    *error = "cannot create directory '" + out_root_ + "': " + ec.message();
    return false;
  }
  std::string manifest_content = kOwnershipManifestHeader;
  for (const std::string& key : keys) {
    manifest_content += key;
    manifest_content.push_back('\n');
  }
  if (const auto existing = llvm::MemoryBuffer::getFile(manifest)) {
    if ((*existing)->getBuffer() == manifest_content) return true;
  }
  std::error_code ec;
  llvm::raw_fd_ostream stream(manifest, ec);
  if (ec) {
    *error =
        "cannot write ownership manifest '" + manifest + "': " + ec.message();
    return false;
  }
  stream << manifest_content;
  return true;
}

}  // namespace tapa::cc
