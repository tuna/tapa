#ifndef TAPA_CODEGEN_TREE_WRITER_H_
#define TAPA_CODEGEN_TREE_WRITER_H_

#include <map>
#include <memory>
#include <set>
#include <string>
#include <vector>

#include "clang/Basic/FileEntry.h"
#include "clang/Basic/SourceLocation.h"
#include "clang/Lex/PPCallbacks.h"
#include "clang/Rewrite/Core/Rewriter.h"

namespace clang {
class ASTContext;
class LangOptions;
class SourceManager;
}  // namespace clang

namespace tapa::cc {

// The rewritten-tree layer of the multi-file frontend (plan §3.3): mirrors
// the user's sources under a work directory so HLS compiles real files at
// real relative locations, rewriting include lines only where the mirror
// moved a file and emitting `#line` markers so diagnostics keep pointing
// at the original sources.
//
// Standalone layer: no production pass drives it yet (the emission wiring
// is the next slice); unit tests are its only consumer.

// One resolved `#include` directive as written in user source.
struct IncludeDirective {
  // The file the directive is written in (canonical absolute path).
  std::string writing_file;
  // The include argument exactly as written, without quotes or brackets.
  std::string spelling;
  // The resolved file (canonical absolute path); empty when unresolved.
  std::string resolved;
  // True for <...>; angle directives are never rewritten.
  bool angled = false;
  // True when the resolution went through a system search entry (-isystem
  // or a system framework dir): such targets are not mirrored, per the
  // "include closure reached through a non-isystem include" rule.
  bool from_system_search = false;
  // The quotes/brackets range in the writing file, driving in-place
  // rewriting. Only valid against the SourceManager of the TU that
  // recorded it.
  clang::CharSourceRange filename_range;
  // The resolved file's entry (null when unresolved), so the writer can
  // fetch the file's bytes without a path re-lookup -- canonical paths do
  // not round-trip through virtual files.
  clang::OptionalFileEntryRef resolved_entry;
};

// Records every inclusion directive resolution of one TU into the log, in
// preprocess order, deduplicated. Attach from BeginSourceFileAction:
//   pp.addPPCallbacks(std::make_unique<IncludeRecorder>(pp.getSourceManager(),
//   &log));
class IncludeRecorder : public clang::PPCallbacks {
 public:
  IncludeRecorder(clang::SourceManager& sm, std::vector<IncludeDirective>* log);

  void InclusionDirective(
      clang::SourceLocation hash_loc, const clang::Token& include_tok,
      llvm::StringRef file_name, bool is_angled,
      clang::CharSourceRange filename_range, clang::OptionalFileEntryRef file,
      llvm::StringRef search_path, llvm::StringRef relative_path,
      const clang::Module* suggested_module, bool module_imported,
      clang::SrcMgr::CharacteristicKind file_type) override;

 private:
  clang::SourceManager& sm_;
  std::vector<IncludeDirective>* log_;
};

// Where each mirrored file lands in the tree. In-root files mirror their
// src-root-relative path (quoted includes keep resolving); out-of-root
// files land in an `_external/<digest8>/<basename>` bucket, one bucket per
// distinct absolute path, digest8 being the first 8 hex chars of the
// sha256 over that path.
class TreeLayout {
 public:
  // The deepest common ancestor directory of the main files (canonical
  // absolute paths); requires at least one input.
  static std::string SrcRoot(const std::vector<std::string>& main_files);

  explicit TreeLayout(std::string src_root);

  const std::string& src_root() const { return src_root_; }

  // Component-boundary prefix test: root "/a" does not contain "/ab/c.h".
  bool InRoot(llvm::StringRef abs_path) const;
  // The src-root-relative key; call only when InRoot().
  std::string InRootKey(llvm::StringRef abs_path) const;
  // The `_external/<digest8>/<basename>` key; call only when !InRoot().
  static std::string ExternalKey(llvm::StringRef abs_path);

  // Registers a mirrored path and reports its tree key. A second distinct
  // path mapping to an already-owned key (a truncated-digest collision, or
  // an in-root file sitting under `_external/`) is a hard error.
  bool Register(const std::string& abs_path, std::string* key,
                std::string* error);

  // The registered key for `abs_path`, or nullptr when unregistered.
  const std::string* KeyOf(llvm::StringRef abs_path) const;

 private:
  std::string src_root_;
  std::map<std::string, std::string> path_to_key_;
  std::map<std::string, std::string> key_to_path_;
};

// One mirrored file under rewrite: its original bytes plus edits, with
// `#line` bookkeeping keeping diagnostics pointed at the original file.
// Every edit that changes the line count is followed by a
// `#line <n> "<abs_path>"` marker re-snapping the following text to its
// original numbering. Markers are computed in original-file coordinates:
// each line-count change re-snaps immediately, so a later edit's marker is
// correct no matter how many earlier edits shifted the text physically.
class TreeFileBuffer {
 public:
  TreeFileBuffer(clang::SourceManager& sm, const clang::LangOptions& lang_opts,
                 clang::FileID file, std::string abs_path);

  // Replaces the character range with `text`. False when the Rewriter
  // rejects the edit (macro locations are not rewritable).
  bool ReplaceWithLineResnap(clang::CharSourceRange range,
                             llvm::StringRef text);

  // Wraps `range` in guard text: `opening` right before it, `closing`
  // right after its last token, each followed by a `#line` re-snap (the
  // task `#ifdef`/`#else`/`#endif` shape; both insertions change the line
  // count by construction).
  bool InsertGuard(clang::SourceRange range, llvm::StringRef opening,
                   llvm::StringRef closing);

  bool had_edits() const { return had_edits_; }

  // The whole file: original bytes when unedited, else the rewritten text.
  std::string Render();

 private:
  // One edit of `length` original bytes at `begin`; emits the re-snap
  // marker when the replacement changes the line count.
  bool ApplyEdit(clang::SourceLocation begin, unsigned length,
                 llvm::StringRef text);

  clang::SourceManager& sm_;
  clang::Rewriter rewriter_;
  clang::FileID file_;
  std::string abs_path_;
  bool had_edits_ = false;
};

// Materializes the mirror tree: which files to mirror (the `-f` inputs
// plus every non-system include target recorded across TUs), where
// (TreeLayout), and with what bytes (a TreeFileBuffer per file rewritten,
// byte-identical copies for the rest). Deterministic: the same inputs,
// logs and edit sequence produce the same bytes, and files whose on-disk
// bytes already match are left untouched.
//
// ASTs never outlive their TU, so the SM-bound work (include rewriting,
// rendering) happens per TU inside AddTu, while that TU's SourceManager
// is still alive -- the same shape as tapacc's index/rewrite passes.
// Write() then works on plain bytes only.
class TreeWriter {
 public:
  // `main_files` are the `-f` inputs as canonical absolute paths;
  // `out_root` is the directory the tree materializes under.
  TreeWriter(std::string out_root, std::vector<std::string> main_files);

  // Absorbs one TU's include log and renders the files that TU owns: its
  // main file, every file its records are written in, and every header it
  // resolved that no earlier TU has rendered. Include rewrites land here.
  // Call once per TU, in input order.
  bool AddTu(clang::ASTContext& ctx, std::vector<IncludeDirective> log,
             std::string* error);

  // Checks the layout for key collisions and writes every rendered file
  // under out_root, creating directories as needed.
  bool Write(std::string* error);

 private:
  std::string out_root_;
  std::vector<std::string> main_files_;
  // Canonical path -> final bytes, in first-sighting order across TUs.
  std::map<std::string, std::string> rendered_;
};

// The canonical form used for every recorded path: the on-disk realpath
// (symlinks resolved), or the path as named when nothing sits there
// (virtual files).
std::string CanonicalPath(llvm::StringRef name);

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_TREE_WRITER_H_
