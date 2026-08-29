#ifndef TAPA_FRONTEND_PROGRAM_BUILDER_H_
#define TAPA_FRONTEND_PROGRAM_BUILDER_H_

#include <map>
#include <optional>
#include <string>
#include <vector>

#include "clang/AST/ASTContext.h"
#include "clang/AST/Decl.h"

#include "nlohmann/json.hpp"

#include "program.h"

namespace tapa::cc {

// Builds one merged task graph out of any number of translation units, in two
// passes over the same file list (each ClangTool action owns its ASTContext,
// so AST nodes never survive their TU):
//
//   pass 1  IndexTu          collect, per TU, every function definition
//                           sighted in the main file plus every invoke edge
//                           (the callee resolved through the declaration
//                           visible in that TU); no rewriting, no JSON.
//   merge   MergeAndDiscover build the merged definition index, BFS from the
//                           top over the merged invoke edges, and check the
//                           merge rules below; pure data, no AST.
//   pass 2  RewriteTu        per TU, build a per-TU Program view of the
//                           merged task set and run the per-task emission
//                           for every merged task defined in that TU.
//
// after both passes, EmitJson prints the single graph tapa-ir consumes.
//
// Merge rules (each with a diagnostic collected into `errors()`):
//   1. a task defined in one TU and invoked from another takes its ports,
//      instances, streams, and rewritten text from the owning TU; an invoke
//      whose callee has no definition anywhere is an error naming the invoke
//      site and the scanned TUs (plus the location of a same-name definition
//      with a different signature, the decl/def mismatch case);
//   2. a task sighted as an identical definition in several TUs (a
//      header-defined task, inlined into every flattened TU) dedupes on
//      identical key + level/target/readable_name/signature; any difference
//      is an error printing both definitions;
//   3. template specializations merge by mangled key; distinct
//      instantiations are distinct tasks;
//   4. an internal-linkage task (`static`, anonymous namespace) is a hard
//      error whether it is task-shaped or referenced by an invoke;
//   5. the top must resolve to exactly one merged definition; "top not
//      found" lists every TU scanned.
class ProgramBuilder {
 public:
  ProgramBuilder(std::string top, SynthTarget default_target);

  // Pass 1. Registers the TU (by main-file path, in first-seen order) and
  // folds its sightings and invoke edges into the shared index.
  void IndexTu(clang::ASTContext& ctx);

  // Merge + discovery. Returns false (and fills `errors()`) on a violated
  // merge rule; the caller must not run pass 2 afterwards.
  bool MergeAndDiscover();

  // Pass 2. Builds the per-TU view and emits the rewritten text of every
  // merged task whose definition this TU owns. Diagnostics (stream wiring,
  // leaked attributes) go through `ctx` as today.
  void RewriteTu(clang::ASTContext& ctx);

  // The per-TU Program view of the merged task set: `file_funcs` /
  // `local_funcs` as today, plus one TaskModel per merged task visible in
  // this TU — the definition if owned here, else a visible redeclaration
  // (headers are inlined into flattened TUs, so cross-TU tasks have decls),
  // else omitted, since its text does not exist in this TU. Ports and
  // instances are filled only for owned tasks (BuildPorts and
  // ParseUpperTask need the definition), which RewriteTu does. Valid only
  // while `ctx` is alive.
  Program TuView(clang::ASTContext& ctx);

  // The merged model of a task by graph name (the JSON `tasks` key), or
  // nullptr. Ports/instances/streams are filled once the owning TU has gone
  // through RewriteTu; the model's AST pointers are TU-local to that call
  // and only its pointer-free fields are read afterwards.
  const TaskModel* FindTask(const std::string& name) const;

  // The rewritten text stored for one task by its owning RewriteTu call.
  std::string TakeTaskCode(const std::string& name) const;

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
    // functions, the mangled name for template specializations (Itanium
    // mangling is identical across TUs for the same specialization).
    std::string key;
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
    TaskModel model;
  };

  // The identity key of a function (see DefSighting::key).
  std::string KeyOf(clang::MangleContext& mangler,
                    const clang::FunctionDecl* func) const;
  // The facts of one definition, pure data with locations pre-rendered.
  DefSighting MakeFacts(clang::MangleContext& mangler,
                        const clang::FunctionDecl* def, int tu) const;

  TuIndex IndexTuImpl(clang::ASTContext& ctx) const;
  int RegisterTu(const std::string& file);
  int TuOf(const std::string& file) const;
  void Fail(std::string message);
  std::string ScannedTus() const;
  const DefSighting& FirstSighting(const std::string& key) const;

  std::string top_;
  SynthTarget default_target_;
  std::vector<std::string> tu_files_;  // TU id -> main-file path

  // key -> sightings, in TU order.
  std::map<std::string, std::vector<DefSighting>> defs_;
  // caller key -> its invoke edges.
  std::multimap<std::string, InvokeEdge> edges_;
  // merged task key -> the task (discovered only).
  std::map<std::string, MergedTask> tasks_;
  // graph name -> rewritten text, stored by the owning RewriteTu call.
  std::map<std::string, std::string> code_;
  std::vector<std::string> errors_;
};

}  // namespace tapa::cc

#endif  // TAPA_FRONTEND_PROGRAM_BUILDER_H_
