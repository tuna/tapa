#include "tree_writer.h"

#include <algorithm>
#include <array>
#include <cassert>
#include <utility>

#include "clang/AST/ASTContext.h"
#include "clang/Basic/FileManager.h"
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
  for (const IncludeDirective& seen : *log_) {
    if (seen.writing_file == record.writing_file &&
        seen.spelling == record.spelling && seen.angled == record.angled &&
        seen.resolved == record.resolved) {
      return;  // the same directive sighted again: keep the first sighting
    }
  }
  log_->push_back(std::move(record));
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

bool TreeLayout::Register(const std::string& abs_path, std::string* key,
                          std::string* error) {
  const std::string assigned =
      InRoot(abs_path) ? InRootKey(abs_path) : ExternalKey(abs_path);
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

TreeFileBuffer::TreeFileBuffer(clang::SourceManager& sm,
                               const clang::LangOptions& lang_opts,
                               clang::FileID file, std::string abs_path)
    : sm_(sm), file_(file), abs_path_(std::move(abs_path)) {
  rewriter_.setSourceMgr(sm_, lang_opts);
}

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

bool TreeFileBuffer::InsertGuard(clang::SourceRange range,
                                 llvm::StringRef opening,
                                 llvm::StringRef closing) {
  const clang::SourceLocation after_last_token =
      clang::Lexer::getLocForEndOfToken(range.getEnd(), 0, sm_,
                                        rewriter_.getLangOpts());
  if (!range.getBegin().isFileID() || !after_last_token.isFileID()) {
    return false;
  }
  return ApplyEdit(range.getBegin(), 0, opening) &&
         ApplyEdit(after_last_token, 0, closing);
}

bool TreeFileBuffer::ApplyEdit(clang::SourceLocation begin, unsigned length,
                               llvm::StringRef text) {
  if (!begin.isFileID() || sm_.getFileID(begin) != file_) return false;
  const llvm::StringRef original = sm_.getBufferData(file_);
  const unsigned begin_offset = sm_.getFileOffset(begin);
  const unsigned end_offset = begin_offset + length;
  const unsigned old_lines =
      llvm::count(original.substr(begin_offset, length), '\n');
  // Insertions go through InsertTextAfter, not ReplaceText(len 0): a
  // replace records its delta at an offset the marker's own mapping then
  // excludes, which would place the marker before the inserted text.
  const bool rejected = length == 0
                            ? rewriter_.InsertTextAfter(begin, text)
                            : rewriter_.ReplaceText(begin, length, text);
  if (rejected) return false;
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
  bool needs_newline =
      text.empty() ? (begin_offset > 0 && original[begin_offset - 1] != '\n')
                   : !text.ends_with('\n');
  if (rest.front() == '\n') {
    // The edit ends exactly at a line boundary: the unchanged newline
    // already terminates the last written line, so pin the NEXT line from
    // its own start, without introducing a blank one.
    if (rest.size() == 1) return true;  // only the final newline follows
    marker_offset = end_offset + 1;
    needs_newline = false;
  }
  const clang::SourceLocation at =
      sm_.getLocForStartOfFile(file_).getLocWithOffset(marker_offset);
  std::string marker;
  llvm::raw_string_ostream os(marker);
  os << (needs_newline ? "\n" : "") << "#line " << sm_.getSpellingLineNumber(at)
     << " \"" << abs_path_ << "\"\n";
  // InsertTextAfter, not Before: an edit at the same offset (a pure
  // insertion's own marker) must land after the inserted text, and
  // RewriteBuffer maps equal offsets through that flag.
  rewriter_.InsertTextAfter(at, marker);
  return true;
}

std::string TreeFileBuffer::Render() {
  if (!had_edits_) return sm_.getBufferData(file_).str();
  std::string out;
  llvm::raw_string_ostream os(out);
  rewriter_.getEditBuffer(file_).write(os);
  return out;
}

// ── The writer ────────────────────────────────────────────────────────────

TreeWriter::TreeWriter(std::string out_root,
                       std::vector<std::string> main_files)
    : out_root_(std::move(out_root)), main_files_(std::move(main_files)) {}

bool TreeWriter::AddTu(clang::ASTContext& ctx,
                       std::vector<IncludeDirective> log, std::string* error) {
  clang::SourceManager& sm = ctx.getSourceManager();
  const TreeLayout layout(TreeLayout::SrcRoot(main_files_));

  // Which files exist in the mirror at all: the `-f` inputs plus every
  // target this TU resolved through a user (non-system) search entry.
  std::set<std::string> mirrored(main_files_.begin(), main_files_.end());
  for (const IncludeDirective& record : log) {
    if (!record.resolved.empty() && !record.from_system_search) {
      mirrored.insert(record.resolved);
    }
  }

  // The bytes of each mirrored file this TU can speak for: its main file,
  // every file its records are written in (rewritten when an include must
  // move to its `_external` spelling), and every header it pulled in that
  // no earlier TU has rendered. First sighting owns the file.
  std::map<std::string, std::unique_ptr<TreeFileBuffer>> buffers;
  std::map<std::string, clang::FileID> plain;
  auto remember = [&](const std::string& path, clang::FileID file) {
    if (rendered_.count(path) > 0 || buffers.count(path) > 0 ||
        plain.count(path) > 0) {
      return;
    }
    plain.emplace(path, file);
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

  // Include-line rewriting: a quoted include of an out-of-root mirror
  // moves to its `_external` spelling (the tree root is on the include
  // path, so it resolves); in-root targets and angle directives keep
  // what the user wrote.
  for (const IncludeDirective& record : log) {
    if (record.angled || record.resolved.empty() || record.from_system_search ||
        layout.InRoot(record.resolved)) {
      continue;
    }
    const auto it = plain.find(record.writing_file);
    if (it == plain.end()) continue;
    if (buffers.count(record.writing_file) == 0) {
      buffers.emplace(
          record.writing_file,
          std::make_unique<TreeFileBuffer>(sm, ctx.getLangOpts(), it->second,
                                           record.writing_file));
    }
    if (!buffers.at(record.writing_file)
             ->ReplaceWithLineResnap(
                 record.filename_range,
                 "\"" + TreeLayout::ExternalKey(record.resolved) + "\"")) {
      *error = "cannot rewrite the include of '" + record.resolved + "' in '" +
               record.writing_file + "'";
      return false;
    }
  }

  // Render: rewritten files through their buffer, the rest byte-identical.
  for (const auto& [path, file] : plain) {
    const auto it = buffers.find(path);
    rendered_[path] = it != buffers.end() ? it->second->Render()
                                          : sm.getBufferData(file).str();
  }
  return true;
}

bool TreeWriter::Write(std::string* error) {
  TreeLayout layout(TreeLayout::SrcRoot(main_files_));
  for (const auto& [path, bytes] : rendered_) {
    (void)bytes;  // registration is about the paths
    std::string key;
    if (!layout.Register(path, &key, error)) return false;
  }

  // Materialize in a fixed order so identical inputs visit the same paths
  // the same way every run; unchanged bytes are skipped.
  for (const auto& [path, content] : rendered_) {
    const std::string key = *layout.KeyOf(path);
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
  return true;
}

}  // namespace tapa::cc
