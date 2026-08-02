#include "invoke_parser.h"

#include <map>
#include <memory>
#include <optional>
#include <string>

#include "clang/AST/Expr.h"
#include "clang/AST/ExprCXX.h"
#include "clang/AST/Mangle.h"

#include "classify.h"
#include "codegen/conventions.h"
#include "diag.h"
#include "discover.h"
#include "names.h"
#include "type_args.h"

namespace tapa::cc {

namespace {

std::optional<int64_t> EvalInt(const clang::ASTContext& ctx,
                               const clang::Expr* expr) {
  clang::Expr::EvalResult result;
  if (expr->EvaluateAsInt(result, ctx)) {
    return result.Val.getInt().getExtValue();
  }
  return std::nullopt;
}

// step (bulk-synchronous mode), vector length, and whether an explicit instance
// name is present, from the invoke method's template specialization arguments.
struct InvokeMode {
  int64_t step = 0;
  uint64_t vec_length = 1;
  bool has_name = false;
};

InvokeMode GetInvokeMode(const clang::CXXMemberCallExpr* invoke) {
  InvokeMode mode;
  const auto* method =
      llvm::dyn_cast_or_null<clang::CXXMethodDecl>(invoke->getCalleeDecl());
  if (method == nullptr) return mode;
  const auto* spec_args = method->getTemplateSpecializationArgs();
  if (spec_args == nullptr) return mode;
  const auto args = spec_args->asArray();
  using TA = clang::TemplateArgument;
  if (!args.empty() && args[0].getKind() == TA::Integral) {
    mode.step = args[0].getAsIntegral().getSExtValue();
  }
  if (args.size() > 1 && args[1].getKind() == TA::Integral) {
    mode.vec_length = args[1].getAsIntegral().getZExtValue();
  }
  if (!args.empty() && args.back().getKind() == TA::Integral) {
    mode.has_name = true;
  }
  return mode;
}

// True when `arg` (possibly an array element like `q[0]`) names one of the
// task's own stream ports — an external FIFO at the kernel boundary.
bool IsStreamPortArg(const TaskModel& task, const std::string& arg) {
  const std::string base = arg.substr(0, arg.find('['));
  for (const Port& port : task.ports) {
    if (port.name != base) continue;
    switch (port.kind) {
      case TapaKind::kIStream:
      case TapaKind::kOStream:
      case TapaKind::kIStreams:
      case TapaKind::kOStreams:
        return true;
      default:
        return false;
    }
  }
  return false;
}

// Look up a stream argument in `task.streams`; for the top task, a stream
// argument that names one of the task's own ports (rather than a locally
// declared FIFO) yields a fresh depth-less entry — an external FIFO whose
// other endpoint is the kernel boundary.
std::map<std::string, StreamDecl>::iterator FindOrExternalStream(
    TaskModel& task, const std::string& arg, bool is_top) {
  auto it = task.streams.find(arg);
  if (it == task.streams.end() && is_top && IsStreamPortArg(task, arg)) {
    it = task.streams
             .emplace(arg, StreamDecl{std::nullopt, nullptr, std::nullopt,
                                      std::nullopt})
             .first;
  }
  return it;
}

// Map an array argument to its i-th element name (wrapping on the array
// length); a scalar argument is returned unchanged.
std::string ArrayElement(const std::string& name, int pos,
                         const clang::DeclRefExpr* ref) {
  if (ref == nullptr) return name;
  const TapaKind rk = ClassifyTapaType(ref->getType());
  if (rk == TapaKind::kMmaps || rk == TapaKind::kIStreams ||
      rk == TapaKind::kOStreams || rk == TapaKind::kStreams) {
    const int64_t len = IntTemplateArg(ref->getType(), 1).value_or(0);
    if (len <= 0) return name;
    return ArrayNameAt(name, pos % len);
  }
  return name;
}

// Shared state for one upper-task parse: the AST/diagnostic context and the
// task model being filled. Endpoint marking lives here (hoisted out of the
// per-argument loop) so the double-consume / double-produce checks sit next
// to the stream map they mutate. Invocation-only state (the child-name
// mangler and the access positions) stays local to `ParseInvocations`.
struct UpperTaskContext {
  clang::ASTContext& ctx;
  TaskModel& task;
  const bool is_top;

  UpperTaskContext(clang::ASTContext& ctx, TaskModel& task, bool is_top)
      : ctx(ctx), task(task), is_top(is_top) {}

  // Record FIFO `a` as consumed by instance `inst_index` of `task_name`,
  // reporting a double-consume at `loc`.
  void MarkConsumer(const std::string& a, const std::string& task_name,
                    uint32_t inst_index, clang::SourceLocation loc) {
    auto it = FindOrExternalStream(task, a, is_top);
    if (it == task.streams.end()) return;
    if (it->second.consumed_by.has_value()) {
      ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error, loc,
                       "tapa::stream '%0' consumed more than once")
          .AddString(a);
    }
    it->second.consumed_by = Endpoint{task_name, inst_index};
  }

  // Record FIFO `a` as produced by instance `inst_index` of `task_name`,
  // reporting a double-produce at `loc`.
  void MarkProducer(const std::string& a, const std::string& task_name,
                    uint32_t inst_index, clang::SourceLocation loc) {
    auto it = FindOrExternalStream(task, a, is_top);
    if (it == task.streams.end()) return;
    if (it->second.produced_by.has_value()) {
      ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error, loc,
                       "tapa::stream '%0' produced more than once")
          .AddString(a);
    }
    it->second.produced_by = Endpoint{task_name, inst_index};
  }
};

// --- 1. Stream (FIFO) declarations. ---
// Scan the task body for `tapa::stream` / `tapa::streams` declarations and
// record each (array elements expanded to `name[i]` entries) in
// `task.streams` with its depth and VarDecl.
void CollectStreamDecls(UpperTaskContext& uc, const clang::FunctionDecl* func) {
  for (const clang::Stmt* child : func->getBody()->children()) {
    const auto* decl_stmt = llvm::dyn_cast<clang::DeclStmt>(child);
    if (decl_stmt == nullptr || !decl_stmt->isSingleDecl()) continue;
    const auto* var =
        llvm::dyn_cast<clang::VarDecl>(decl_stmt->getSingleDecl());
    if (var == nullptr) continue;
    const std::string name = var->getNameAsString();
    switch (ClassifyTapaType(var->getType())) {
      case TapaKind::kStream:
        uc.task.streams[name] =
            StreamDecl{static_cast<uint64_t>(
                           IntTemplateArg(var->getType(), 1).value_or(0)),
                       var, std::nullopt, std::nullopt};
        break;
      case TapaKind::kStreams: {
        const uint64_t depth = IntTemplateArg(var->getType(), 2).value_or(0);
        const int64_t n = IntTemplateArg(var->getType(), 1).value_or(0);
        for (int64_t i = 0; i < n; ++i) {
          uc.task.streams[ArrayNameAt(name, i)] =
              StreamDecl{depth, var, std::nullopt, std::nullopt};
        }
        break;
      }
      default:
        break;
    }
  }
}

// --- 2. Invocations. ---
// Walk `task_obj`'s `tapa::task::invoke(...)` chain: resolve the callee task,
// push one `Instance` per vector lane, and bind each argument to its callee
// port — distributing array arguments across scalar ports by access position
// and marking each stream endpoint as it is bound.
void ParseInvocations(UpperTaskContext& uc, const clang::Expr* task_obj) {
  // The child-name mangler and the access positions, which distribute an
  // array argument (streams/mmaps) across the scalar ports it feeds, in
  // order of appearance.
  const auto mangler = CreateMangleContext(uc.ctx);
  std::map<std::string, int> istreams_pos;
  std::map<std::string, int> ostreams_pos;
  std::map<std::string, int> mmaps_pos;
  std::map<const clang::Expr*, int> seq_pos;

  for (const clang::CXXMemberCallExpr* invoke : GetInvokes(task_obj)) {
    const InvokeMode mode = GetInvokeMode(invoke);
    bool has_executable = false;
    std::string task_name;
    const clang::FunctionDecl* callee = nullptr;

    for (uint64_t i_vec = 0; i_vec < mode.vec_length; ++i_vec) {
      for (unsigned i = 0; i < invoke->getNumArgs(); ++i) {
        const clang::Expr* arg = invoke->getArg(i);

        if (ClassifyTapaType(arg->getType()) == TapaKind::kExecutable) {
          has_executable = true;
          continue;
        }

        const auto* decl_ref = llvm::dyn_cast<clang::DeclRefExpr>(arg);
        const auto* mat = llvm::dyn_cast<clang::MaterializeTemporaryExpr>(arg);
        const auto* op_call =
            mat ? llvm::dyn_cast<clang::CXXOperatorCallExpr>(mat->getSubExpr())
                : nullptr;
        const bool is_seq = ClassifyTapaType(arg->getType()) == TapaKind::kSeq;
        const std::optional<int64_t> as_int = EvalInt(uc.ctx, arg);
        const bool is_int_literal =
            !decl_ref && !op_call && !is_seq && as_int.has_value();

        std::string arg_name;
        if (decl_ref != nullptr) {
          arg_name = decl_ref->getNameInfo().getAsString();
        } else if (op_call != nullptr) {
          const auto* base = llvm::dyn_cast<clang::DeclRefExpr>(
              op_call->getArg(0)->IgnoreImplicit());
          const int64_t idx = EvalInt(uc.ctx, op_call->getArg(1)).value_or(0);
          if (base != nullptr) {
            arg_name = ArrayNameAt(base->getNameInfo().getAsString(), idx);
          }
        } else if (is_int_literal) {
          arg_name = "64'd" + std::to_string(static_cast<uint64_t>(*as_int));
        }

        if (decl_ref != nullptr || op_call != nullptr || is_int_literal ||
            is_seq) {
          if (i == 0) {
            callee = decl_ref == nullptr
                         ? nullptr
                         : llvm::dyn_cast_or_null<clang::FunctionDecl>(
                               decl_ref->getDecl()->getAsFunction());
            if (callee == nullptr) break;  // not a task reference
            task_name = TaskName(*mangler, callee);
            uc.task.instances[task_name].push_back(
                Instance{callee, task_name, mode.step, std::nullopt, {}});
            continue;
          }
          if (callee == nullptr) continue;

          const int skip = (mode.has_name ? 1 : 0) + (has_executable ? 1 : 0);
          const int param_idx = static_cast<int>(i) - 1 - skip;
          if (param_idx < 0 ||
              param_idx >= static_cast<int>(callee->getNumParams())) {
            continue;
          }
          const clang::ParmVarDecl* param = callee->getParamDecl(param_idx);
          const std::string port = param->getNameAsString();
          const TapaKind pk = ClassifyTapaType(param);

          std::vector<Instance>& insts = uc.task.instances[task_name];
          const uint32_t inst_index = static_cast<uint32_t>(insts.size() - 1);

          auto set_arg = [&](const std::string& a, const std::string& p,
                             TapaKind cat) {
            insts.back().args[p] = Arg{a, cat};
          };

          if (pk == TapaKind::kMmap || pk == TapaKind::kImmap ||
              pk == TapaKind::kOmmap || pk == TapaKind::kAsyncMmap) {
            set_arg(ArrayElement(arg_name, mmaps_pos[arg_name]++, decl_ref),
                    port, pk);
          } else if (pk == TapaKind::kIStream) {
            const std::string a =
                ArrayElement(arg_name, istreams_pos[arg_name]++, decl_ref);
            uc.MarkConsumer(a, task_name, inst_index, arg->getBeginLoc());
            set_arg(a, port, TapaKind::kIStream);
          } else if (pk == TapaKind::kOStream) {
            const std::string a =
                ArrayElement(arg_name, ostreams_pos[arg_name]++, decl_ref);
            uc.MarkProducer(a, task_name, inst_index, arg->getBeginLoc());
            set_arg(a, port, TapaKind::kOStream);
          } else if (pk == TapaKind::kIStreams) {
            const int64_t n = IntTemplateArg(param->getType(), 1).value_or(0);
            for (int64_t j = 0; j < n; ++j) {
              const std::string a =
                  ArrayElement(arg_name, istreams_pos[arg_name]++, decl_ref);
              uc.MarkConsumer(a, task_name, inst_index, arg->getBeginLoc());
              set_arg(a, ArrayNameAt(port, j), TapaKind::kIStream);
            }
          } else if (pk == TapaKind::kOStreams) {
            const int64_t n = IntTemplateArg(param->getType(), 1).value_or(0);
            for (int64_t j = 0; j < n; ++j) {
              const std::string a =
                  ArrayElement(arg_name, ostreams_pos[arg_name]++, decl_ref);
              uc.MarkProducer(a, task_name, inst_index, arg->getBeginLoc());
              set_arg(a, ArrayNameAt(port, j), TapaKind::kOStream);
            }
          } else if (is_seq) {
            set_arg("64'd" + std::to_string(seq_pos[arg]++), port,
                    TapaKind::kNotTapa);
          } else {
            set_arg(arg_name, port, TapaKind::kNotTapa);  // scalar
          }
          continue;
        }

        if (const auto* str = llvm::dyn_cast<clang::StringLiteral>(arg)) {
          if (i == 1 && mode.has_name && !task_name.empty()) {
            uc.task.instances[task_name].back().name = str->getString().str();
            continue;
          }
        }

        ReportCustomDiag(uc.ctx, clang::DiagnosticsEngine::Error,
                         arg->getBeginLoc(), "unexpected argument: %0")
            .AddString(arg->getStmtClassName());
      }
    }
  }
}

// --- 3. Stream validation. ---
// Post-pass over the collected streams: prune unused FIFOs (warning) and
// report declared FIFOs with an unbalanced producer/consumer (error).
void ValidateStreams(UpperTaskContext& uc) {
  for (auto it = uc.task.streams.begin(); it != uc.task.streams.end();) {
    if (it->second.decl == nullptr) {
      // External top-level stream port: exactly one endpoint by construction
      // (the other side is the kernel boundary), so nothing to validate.
      ++it;
      continue;
    }
    const bool produced = it->second.produced_by.has_value();
    const bool consumed = it->second.consumed_by.has_value();
    const clang::SourceLocation loc = it->second.decl != nullptr
                                          ? it->second.decl->getBeginLoc()
                                          : clang::SourceLocation();
    if (!produced && !consumed) {
      ReportCustomDiag(uc.ctx, clang::DiagnosticsEngine::Warning, loc,
                       "unused stream: %0")
          .AddString(it->first);
      it = uc.task.streams.erase(it);
    } else {
      if (produced != consumed) {
        if (consumed) {
          ReportCustomDiag(uc.ctx, clang::DiagnosticsEngine::Error, loc,
                           "consumed but not produced stream: %0")
              .AddString(it->first);
        } else {
          ReportCustomDiag(uc.ctx, clang::DiagnosticsEngine::Error, loc,
                           "produced but not consumed stream: %0")
              .AddString(it->first);
        }
      }
      ++it;
    }
  }
}

}  // namespace

void ParseUpperTask(clang::ASTContext& ctx, TaskModel& task, bool is_top) {
  const clang::FunctionDecl* func = task.def;
  if (func == nullptr || !func->hasBody()) return;
  const clang::Expr* task_obj = GetTapaTaskObject(func->getBody());
  if (task_obj == nullptr) return;  // leaf task

  UpperTaskContext uc(ctx, task, is_top);
  CollectStreamDecls(uc, func);
  ParseInvocations(uc, task_obj);
  ValidateStreams(uc);
}

}  // namespace tapa::cc
