#ifndef TAPA_FRONTEND_PROGRAM_BUILDER_H_
#define TAPA_FRONTEND_PROGRAM_BUILDER_H_

#include <map>
#include <memory>
#include <set>
#include <string>
#include <vector>

#include "clang/AST/ASTContext.h"
#include "clang/AST/Decl.h"
#include "clang/Tooling/Syntax/Tokens.h"

#include "nlohmann/json.hpp"

#include "program.h"

namespace tapa::cc {

struct IncludeDirective;
class TreeWriter;

// Builds one merged task graph out of any number of translation units, in two
// passes over the same file list (each ClangTool action owns its ASTContext,
// so AST nodes never survive their TU):
//
//   pass 1  IndexTu          collect definitions and invoke edges from every
//                            mirrored user file of the TU; no rewriting, no
//                            JSON.
//   merge   MergeAndDiscover build the merged definition index, BFS from the
//                            top over the merged invoke edges, and check the
//                            merge rules below; pure data, no AST.
//   pass 2  RewriteTreeTu    build a per-TU Program view and apply one
//                            guarded rewrite across the mirrored files.
//
// after both passes, EmitJson prints the single graph tapa-ir consumes.
//
// Merge rules (each with a diagnostic collected into `errors()`):
//   1. a task defined in one TU and invoked from another takes its ports,
//      instances, streams, and rewritten text from the owning TU; an invoke
//      whose callee has no definition anywhere is an error naming the invoke
//      site and the scanned TUs (plus the location of a same-name definition
//      with a different signature, the decl/def mismatch case);
//   2. a header-defined task dedupes by its identity key (FQN + canonical
//      signature, so a TU that invokes through a bare declaration matches
//      the TU that owns the definition); divergent sightings are an error
//      printing both definitions, and a task defined at two distinct sites
//      is an error printing both locations -- one mirror serves every TU,
//      so a task must be defined in exactly one place;
//   3. template specializations merge by mangled key; distinct
//      instantiations are distinct tasks;
//   4. an internal-linkage task (`static`, anonymous namespace) is a hard
//      error whether it is task-shaped or referenced by an invoke;
//   5. the top must resolve to exactly one merged definition; "top not
//      found" lists every TU scanned.
struct TreeConfig {
  std::string out_root;
  std::vector<std::string> main_files;
};

class ProgramBuilder {
 public:
  ProgramBuilder(std::string top, SynthTarget default_target, TreeConfig tree);
  ~ProgramBuilder();
  ProgramBuilder(ProgramBuilder&&) noexcept;
  ProgramBuilder& operator=(ProgramBuilder&&) noexcept;

  // Pass 1. Registers the TU (by main-file path, in first-seen order) and
  // folds its sightings and invoke edges into the shared index. `log` is
  // the TU's recorded include directives: they decide which files are user
  // code and therefore indexed and mirrored.
  void IndexTu(clang::ASTContext& ctx,
               const std::vector<IncludeDirective>& log);

  // Merge + discovery. Returns false (and fills `errors()`) on a violated
  // merge rule; the caller must not run pass 2 afterwards.
  bool MergeAndDiscover();

  // Pass 2: apply one guarded rewrite across the mirror and absorb the TU
  // while its SourceManager-bound include log is alive. `tokens` is the
  // TU's recorded expanded-token stream (null when this run recorded none);
  // rewrite edits anchored inside macro expansions compose into its
  // invocation splices.
  void RewriteTreeTu(clang::ASTContext& ctx, std::vector<IncludeDirective> log,
                     const clang::syntax::TokenBuffer* tokens);

  // The per-TU Program view of the merged task set: `file_funcs` /
  // `local_funcs` of the TU's mirrored files, plus one TaskModel per merged
  // task visible in this TU — the definition if owned here, else a visible
  // redeclaration, else omitted since its text does not exist in this TU's
  // selected source files. Ports and instances are filled only for owned
  // tasks (BuildPorts and ParseUpperTask need the definition), which the
  // rewrite pass does. Valid only while `ctx` is alive.
  Program TuView(clang::ASTContext& ctx,
                 const std::vector<IncludeDirective>& log);

  // The merged model of a task by graph name (the JSON `tasks` key), or
  // nullptr. Ports/instances/streams are filled once the owning TU has gone
  // through RewriteTreeTu; the model's AST pointers are TU-local to that
  // call and only its pointer-free fields are read afterwards.
  const TaskModel* FindTask(const std::string& name) const;

  // Materializes the guarded mirror accumulated by RewriteTreeTu.
  bool WriteTree(std::string* error);

  // The single merged task graph, in today's field order and spelling.
  nlohmann::json EmitJson() const;

  const std::string& top() const { return top_; }

  // Merge-rule violations, one entry per failed rule check. The owner of
  // this class prints them; the builder itself stays silent so tests can
  // assert on the exact messages.
  const std::vector<std::string>& errors() const { return errors_; }

 private:
  // One function definition sighted in one TU, as pure data: AST pointers
  // are dropped at the end of IndexTu, so the merge never touches dead state.
  struct DefSighting {
    // Identity key: FQN + canonical parameter signature for plain
    // functions, the mangled name for template specializations. The same
    // spelling in every TU: a cross-TU invoke resolves through a
    // declaration that has no definition in its own TU, so the key cannot
    // carry anything TU-local.
    std::string key;
    // The definition's canonical path + file offset, the site identity: one
    // key sighted at two distinct sites is one function defined twice,
    // which a single mirrored tree cannot honor.
    std::string site;
    // For a template specialization: the identity key of its primary
    // template pattern, so a TU that never instantiates the task can still
    // find (and identically guard) the shared header that defines the
    // pattern.
    std::string primary_key;
    std::string name;        // graph key: plain name, mangled for specs
    std::string plain_name;  // unqualified name, the old lookup key
    std::string readable_name;
    std::string fqn;
    std::string signature;  // canonical parameter types, joined with ","
    std::string loc;        // rendered file:line:col of the definition
    int tu = 0;
    bool is_spec = false;
    bool implicit_spec = false;  // an implicit instantiation, only sighted
                                 // through the invoke that triggered it
    bool task_shaped = false;    // the body builds a tapa::task
    bool ignored = false;        // [[tapa::target("ignore")]]
    bool internal = false;       // static / anonymous namespace / a method
    TaskLevel level = TaskLevel::kLower;
    SynthTarget target = SynthTarget::kXilinxHls;
  };

  // One invoke edge, pure data: the caller and callee identity keys plus the
  // rendered invoke site for diagnostics.
  struct InvokeEdge {
    std::string caller_key;
    std::string callee_key;
    std::string callee_fqn;
    std::string callee_name;
    std::string caller_name;
    std::string loc;
    int tu = 0;
  };

  // The per-TU index, transient to one AST.
  struct TuIndex {
    // Identity key -> a decl visible in this TU; a definition outranks a
    // declaration when both exist.
    std::map<std::string, const clang::FunctionDecl*> visible;
    std::vector<DefSighting> defs;  // in source order
    std::vector<InvokeEdge> invokes;
  };

  // One merged task: the sightings agreed on one definition (the first, in
  // TU order, owns the model), plus the graph data pass 2 computes there.
  struct MergedTask {
    int owner_tu = 0;
    // Template specializations carry their invoking upper task, so the
    // mangled wrapper is emitted right after it (where the invoke is).
    std::optional<std::string> invoker_key;
    // The primary-template key of a specialization (see DefSighting).
    std::string primary_key;
    TaskModel model;
  };

  // The identity key of a function (see DefSighting::key).
  std::string KeyOf(clang::MangleContext& mangler,
                    const clang::FunctionDecl* func) const;
  // The facts of one definition, pure data with locations pre-rendered.
  DefSighting MakeFacts(clang::MangleContext& mangler,
                        const clang::FunctionDecl* def, int tu) const;

  TuIndex IndexTuImpl(clang::ASTContext& ctx,
                      const std::set<clang::FileID>* files) const;
  std::set<clang::FileID> TreeFiles(
      clang::ASTContext& ctx, const std::vector<IncludeDirective>& log) const;
  void FillOwnedTasks(int tu, Program& view, clang::ASTContext& ctx);
  int RegisterTu(const std::string& file);
  int TuOf(const std::string& file) const;
  // The shared file name of one TU, and the merge-time check that TU
  // basenames stay distinct so those names cannot collide.
  std::string SharedSourceName(int tu) const;
  bool CheckSharedFileNames();
  void Fail(std::string message);
  std::string ScannedTus() const;
  const DefSighting& FirstSighting(const std::string& key) const;

  std::string top_;
  SynthTarget default_target_;
  TreeConfig tree_config_;
  std::unique_ptr<TreeWriter> tree_writer_;
  std::vector<std::string> tu_files_;  // TU id -> main-file path

  // key -> sightings, in TU order.
  std::map<std::string, std::vector<DefSighting>> defs_;
  // caller key -> its invoke edges.
  std::multimap<std::string, InvokeEdge> edges_;
  // merged task key -> the task (discovered only).
  std::map<std::string, MergedTask> tasks_;
  std::vector<std::string> errors_;
};

}  // namespace tapa::cc

#endif  // TAPA_FRONTEND_PROGRAM_BUILDER_H_
