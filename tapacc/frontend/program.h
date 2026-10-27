#ifndef TAPA_FRONTEND_PROGRAM_H_
#define TAPA_FRONTEND_PROGRAM_H_

#include <cstdint>
#include <map>
#include <optional>
#include <set>
#include <string>
#include <vector>

#include "clang/AST/Decl.h"

#include "classify.h"

namespace tapa::cc {

// Where a task is in the dataflow hierarchy.
enum class TaskLevel { kUpper, kLower };

// The synthesis target resolved for a task: the tool-wide `--target` unless a
// `[[tapa::target(...)]]` attribute or a `--task-target` mapping overrides
// it. The Xilinx flows have distinct values but share one Vitis backend.
// `kAie` puts the task on the AI Engine array.
enum class SynthTarget {
  kXilinxHls,
  kXilinxVitis,
  kXilinxSlash,
  kIgnore,
  kAie
};

// One external port of a task (an entry in the emitted task graph's "ports").
struct Port {
  std::string name;
  TapaKind kind;      // serialized "cat" via TapaKindCat(kind)
  std::string ctype;  // C++ type string, e.g. "const float*", "uint64_t"
  uint32_t width = 0;
  std::optional<uint32_t> chan_count;  // hmap / istreams / ostreams
  std::optional<uint32_t> chan_size;   // hmap
  std::optional<uint32_t> aie_frame;   // [[tapa::aie_frame(N)]]
  std::optional<uint32_t> aie_plio;    // [[tapa::aie_plio(W)]]
};

// A `[[tapa::aie_location(col, row[, instance])]]` tile pin of an AI Engine
// task; `instance` picks one invocation of a task invoked several times.
struct AieLocation {
  uint32_t col = 0;
  uint32_t row = 0;
  std::optional<uint32_t> instance;
};

// One argument of a child invocation: what the child port binds to in the
// parent's scope, plus the category of that child port. A binding is either a
// name (port / FIFO) or an integer constant converted to the child port type.
// The constant's bits are reported as a width and a value; spelling them as a
// Verilog literal is the RTL backend's job, not the frontend's.
struct Arg {
  std::string arg;                // empty when `value` is set
  std::optional<uint64_t> value;  // converted constant bits
  uint32_t width = 0;             // bit width of `value`
  TapaKind cat;

  static Arg Named(std::string name, TapaKind cat) {
    return Arg{std::move(name), std::nullopt, 0, cat};
  }
  static Arg Constant(uint64_t value, uint32_t width, TapaKind cat) {
    return Arg{"", value, width, cat};
  }
};

// One instantiation of a child task within an upper task (post vec-expansion).
struct Instance {
  const clang::FunctionDecl* callee = nullptr;
  std::string task_name;            // mangled name for template specializations
  int64_t step = 0;                 // join=0, detach=-1, sequential>=1
  std::optional<std::string> name;  // explicit invoke("name", ...)
  std::map<std::string, Arg> args;  // child-port name -> binding
};

// A producer/consumer reference: the child instance that drives or drains a
// FIFO, identified by task name and its index within that task's instance list
// (serialized as the `[task, index]` endpoint pair).
struct Endpoint {
  std::string task;
  uint32_t index = 0;
};

// A FIFO inside an upper task: either a declared `tapa::stream`/`tapa::streams`
// (depth set, `decl` points at the VarDecl) or, at the top level only, a stream
// port of the top task bound to a child (no depth, null `decl`) — an "external"
// FIFO whose other endpoint is the kernel boundary.
struct StreamDecl {
  std::optional<uint64_t> depth;
  const clang::VarDecl* decl = nullptr;
  std::optional<Endpoint> produced_by;
  std::optional<Endpoint> consumed_by;
};

// The typed model of one TAPA task. Built by the frontend with no source
// rewriting; consumed by codegen and (later) the JSON serializer. Every field
// is computed once here, so level/target/name are never recomputed downstream.
struct TaskModel {
  const clang::FunctionDecl* def = nullptr;  // the definition
  // The function whose body instantiates this task; the mangled wrapper for a
  // template specialization is emitted right after it. Null for non-specialized
  // tasks.
  const clang::FunctionDecl* invoker = nullptr;
  bool is_template_spec = false;

  std::string name;           // task key (mangled for template specializations)
  std::string readable_name;  // pretty (templated) name; == name otherwise
  TaskLevel level = TaskLevel::kLower;
  SynthTarget target = SynthTarget::kXilinxHls;

  std::vector<Port> ports;
  std::vector<AieLocation> aie_locations;
  // AI Engine leaf tasks only: the bytes of the body's local variables,
  // which the kernel keeps on its stack.
  uint32_t aie_stack_bytes = 0;
  // Stream ports the body closes (`close()`/`try_close()`): an end-of-
  // transmission token must never cross into an AI Engine task.
  std::set<std::string> closed_ports;

  // Upper tasks only: child instantiations keyed by child task name, and FIFOs
  // keyed by variable name. Ordered maps -> deterministic output (§8.5), never
  // pointer-keyed.
  std::map<std::string, std::vector<Instance>> instances;
  std::map<std::string, StreamDecl> streams;
};

// The whole program: the top-level task name, the ordered list of global
// functions defined in the main file (drives codegen's per-task file assembly),
// and every TAPA task reachable from the top, keyed by name.
struct Program {
  std::string top;
  std::vector<const clang::FunctionDecl*> file_funcs;
  // Internal-linkage helpers and methods: not tasks, but they still take the
  // inline policy so the vendor cannot choose their hierarchy for us.
  std::vector<const clang::FunctionDecl*> local_funcs;
  std::map<std::string, TaskModel> tasks;
};

}  // namespace tapa::cc

#endif  // TAPA_FRONTEND_PROGRAM_H_
