#include "invoke_parser.h"

#include <limits>
#include <map>
#include <optional>
#include <set>
#include <string>
#include <vector>

#include "clang/AST/Expr.h"
#include "clang/AST/ExprCXX.h"
#include "clang/Lex/Lexer.h"
#include "llvm/ADT/APFloat.h"
#include "llvm/ADT/APSInt.h"

#include "classify.h"
#include "codegen/conventions.h"
#include "diag.h"
#include "discover.h"
#include "names.h"
#include "type_args.h"

namespace tapa::cc {

namespace {

// The element an indexed channel array argument (`q[2]`) names: hardware
// binds one fixed channel, so the index must be an integer constant inside
// the array, or nullopt.
std::optional<std::string> ArrayElementArg(
    const clang::ASTContext& ctx, const clang::CXXOperatorCallExpr* op) {
  const auto* base =
      llvm::dyn_cast<clang::DeclRefExpr>(op->getArg(0)->IgnoreImplicit());
  if (base == nullptr || op->getNumArgs() != 2) return std::nullopt;
  const std::optional<llvm::APSInt> index =
      op->getArg(1)->getIntegerConstantExpr(ctx);
  const int64_t length = IntTemplateArg(base->getType(), 1).value_or(0);
  if (!index || index->isNegative() || index->getActiveBits() > 63 ||
      index->getExtValue() >= length) {
    return std::nullopt;
  }
  return ArrayNameAt(base->getNameInfo().getAsString(),
                     static_cast<int>(index->getExtValue()));
}

// Whether an evaluated value survives the task graph's 64-bit constant
// form whole. Check before resizing the evaluated integer; otherwise a
// wider constant would serialize with a truthful-looking width and the
// wrong bits.
bool FitsIn64Bits(const clang::Expr::EvalResult& result) {
  const llvm::APSInt& value = result.Val.getInt();
  // Signed values need their sign bit as well.
  return value.isNegative() ? value.getSignificantBits() <= 64
                            : value.getActiveBits() <= 64;
}

// Convert `value` to `type` before serializing its bits. Integral ports
// extend or truncate the value; float and double ports take the numeric
// conversion using the target's floating semantics. Preserve the source's
// signedness so unsigned high bits are not mistaken for negative values.
uint64_t ConvertToPortType(const clang::ASTContext& ctx,
                           const llvm::APSInt& value, clang::QualType type) {
  // A boolean conversion is not an integral one: every nonzero value becomes
  // `true`, rather than the low bit surviving a truncation.
  if (type->isBooleanType()) return !value.isZero() ? 1 : 0;
  if (type->isSpecificBuiltinType(clang::BuiltinType::Float) ||
      type->isSpecificBuiltinType(clang::BuiltinType::Double)) {
    llvm::APFloat converted(ctx.getFloatTypeSemantics(type));
    converted.convertFromAPInt(value, value.isSigned(),
                               llvm::APFloat::rmNearestTiesToEven);
    return converted.bitcastToAPInt().getZExtValue();
  }
  // `tapa::u<W>` and `tapa::i<W>` wrap a value to W bits, as an integral
  // conversion does; their object is wider than W, so the bits above W
  // must not survive either.
  if (const std::optional<uint32_t> width = ArbitraryIntWidth(type);
      width && *width <= 64) {
    return value.extOrTrunc(*width).getZExtValue();
  }
  const uint64_t bits = value.extOrTrunc(64).getZExtValue();
  if (!type->isIntegralOrEnumerationType()) return bits;
  llvm::APSInt converted(llvm::APInt(/*numBits=*/64, bits),
                         /*isUnsigned=*/true);
  converted = converted.extOrTrunc(ctx.getIntWidth(type));
  converted.setIsSigned(type->isSignedIntegerOrEnumerationType());
  return converted.getZExtValue();
}

// The width of an integer whose bits forward raw: a non-boolean builtin
// integer's, or an arbitrary-precision integer's declared W.
std::optional<uint32_t> ForwardableIntWidth(const clang::ASTContext& ctx,
                                            clang::QualType type) {
  if (type->isIntegerType()) {
    if (type->isBooleanType()) return std::nullopt;
    return static_cast<uint32_t>(ctx.getIntWidth(type));
  }
  return ArbitraryIntWidth(type);
}

// Both control pipelines forward raw scalar bits. Distributed control packs
// each source into a field sized for the child, so even integral resizing
// requires a conversion that the generated wiring does not provide.
bool CanForwardScalar(const clang::ASTContext& ctx, clang::QualType source,
                      clang::QualType destination) {
  if (ctx.hasSameUnqualifiedType(source, destination)) return true;
  const std::optional<uint32_t> from = ForwardableIntWidth(ctx, source);
  const std::optional<uint32_t> to = ForwardableIntWidth(ctx, destination);
  return from && to && *from == *to;
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

  // Key the mode off the overload's own shape, never off flat
  // template-argument positions: an empty Args pack contributes nothing to
  // the flat list, so a name-only invoke's name_size would otherwise be
  // misread as the vector length — spawning one instance per name
  // character instead of one instance, named.
  if (const auto* fun_template = method->getPrimaryTemplate()) {
    bool step_seen = false;
    const auto params = fun_template->getTemplateParameters()->asArray();
    for (unsigned pos = 0; pos < params.size() && pos < args.size(); ++pos) {
      const auto* nttp =
          llvm::dyn_cast<clang::NonTypeTemplateParmDecl>(params[pos]);
      if (nttp == nullptr || args[pos].getKind() != TA::Integral) continue;
      const clang::QualType type = nttp->getType();
      if (type->isEnumeralType()) {
        // The InvokeMode argument (join/detach/sequential).
        mode.step = args[pos].getAsIntegral().getSExtValue();
        step_seen = true;
      } else if (type->isSpecificBuiltinType(clang::BuiltinType::Int)) {
        // The stub spells the mode int and follows it with n; the real
        // headers spell it InvokeMode and follow it with n. Either way the
        // first int non-type parameter is the mode and the second is n; a
        // size_t parameter is the name length and carries no mode.
        if (!step_seen) {
          mode.step = args[pos].getAsIntegral().getSExtValue();
          step_seen = true;
        } else {
          mode.vec_length = args[pos].getAsIntegral().getZExtValue();
        }
      }
    }
  }

  mode.has_name = InvokeNamesInstance(invoke);
  return mode;
}

// True when `arg` (possibly an array element like `q[0]`) names one of the
// task's own stream ports — an external FIFO at the kernel boundary.
bool IsStreamPortArg(const TaskModel& task, const std::string& arg) {
  const Port* port = FindPort(task, ArrayBaseName(arg));
  return port != nullptr && IsStreamInterface(port->kind);
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
    if (MarkPassedPort(a, loc)) return;
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
    if (MarkPassedPort(a, loc)) return;
    auto it = FindOrExternalStream(task, a, is_top);
    if (it == task.streams.end()) return;
    if (it->second.produced_by.has_value()) {
      ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error, loc,
                       "tapa::stream '%0' produced more than once")
          .AddString(a);
    }
    it->second.produced_by = Endpoint{task_name, inst_index};
  }

  // Record that the task hands its own async_mmap parameter `a` to a task,
  // reporting a second hand-off at `loc`: tasks sharing one async_mmap would
  // share its channels in software simulation, but each gets its own on
  // hardware.
  void MarkHandedOn(const std::string& a, clang::SourceLocation loc) {
    if (handed_on.insert(a).second) return;
    ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error, loc,
                     "tapa::async_mmap '%0' is handed to more than one task; "
                     "take a tapa::mmap<T> instead, which each task takes "
                     "as its own async_mmap")
        .AddString(a);
  }

 private:
  // A non-top task's own stream port `a` is wired through to its task by
  // name rather than as a FIFO, yet it is still one stream with one reader
  // and one writer; report a second task taking it at `loc`. False when
  // `a` is no such port.
  bool MarkPassedPort(const std::string& a, clang::SourceLocation loc) {
    if (is_top || !IsStreamPortArg(task, a)) return false;
    if (!passed_ports.insert(a).second) {
      ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error, loc,
                       "stream port '%0' is passed to more than one task; a "
                       "stream has exactly one reader and one writer")
          .AddString(a);
    }
    return true;
  }

  std::set<std::string> handed_on;
  std::set<std::string> passed_ports;
};

// Recursively reject stream declarations below the top level of the task
// body. `CollectStreamDecls` only sees direct children, so a stream
// declared inside an `if`/loop/lambda would otherwise be invisible to
// codegen — its invokes then failing in confusing downstream ways or not
// at all.
void DiagnoseNestedStreamDecls(UpperTaskContext& uc, const clang::Stmt* stmt) {
  if (stmt == nullptr) return;
  if (const auto* decl_stmt = llvm::dyn_cast<clang::DeclStmt>(stmt)) {
    for (const clang::Decl* d : decl_stmt->decls()) {
      const auto* var = llvm::dyn_cast<clang::VarDecl>(d);
      if (var == nullptr) continue;
      switch (ClassifyTapaType(var->getType())) {
        case TapaKind::kStream:
        case TapaKind::kStreams:
          ReportCustomDiag(uc.ctx, clang::DiagnosticsEngine::Error,
                           var->getLocation(),
                           "tapa::stream '%0' must be declared at the top "
                           "level of the task body")
              .AddString(var->getNameAsString());
          break;
        default:
          break;
      }
    }
    return;
  }
  for (const clang::Stmt* child : stmt->children()) {
    DiagnoseNestedStreamDecls(uc, child);
  }
}

// --- 0. The body's shape. ---
// Hardware keeps only an upper task's channel declarations and invokes, so a
// statement that computes would run in software simulation and silently
// vanish from the RTL; an upper task instantiated from a template is not
// lowered at all. Report both. Constants (constexpr, or const with a constant
// initializer), declarations that declare no variable, and `assert`s, which
// hardware drops as an NDEBUG build does, compute nothing and stay allowed.
bool IsPassiveDecl(clang::ASTContext& ctx, const clang::Decl* decl) {
  const auto* var = llvm::dyn_cast<clang::VarDecl>(decl);
  if (var == nullptr) return true;
  if (ClassifyTapaType(var->getType()) != TapaKind::kNotTapa) return true;
  if (var->isConstexpr()) return true;
  const clang::Expr* init = var->getInit();
  return var->getType().isConstQualified() && init != nullptr &&
         init->isEvaluatable(ctx);
}

bool IsPassiveStmt(clang::ASTContext& ctx, const clang::Stmt* stmt) {
  if (llvm::isa<clang::NullStmt>(stmt)) return true;
  const clang::SourceLocation begin = stmt->getBeginLoc();
  if (begin.isMacroID() &&
      clang::Lexer::getImmediateMacroName(begin, ctx.getSourceManager(),
                                          ctx.getLangOpts()) == "assert") {
    return true;
  }
  if (const auto* ret = llvm::dyn_cast<clang::ReturnStmt>(stmt)) {
    return ret->getRetValue() == nullptr;
  }
  if (const auto* decls = llvm::dyn_cast<clang::DeclStmt>(stmt)) {
    for (const clang::Decl* decl : decls->decls()) {
      if (!IsPassiveDecl(ctx, decl)) return false;
    }
    return true;
  }
  const auto* expr = llvm::dyn_cast<clang::Expr>(stmt);
  return expr != nullptr &&
         ClassifyTapaType(expr->getType()) == TapaKind::kTask;
}

void DiagnoseUpperBody(UpperTaskContext& uc, const clang::FunctionDecl* func) {
  // An explicit specialization spells a body of its own, which is lowered
  // like any upper task's; an instantiated body is not.
  if (func->isTemplateInstantiation() ||
      func->getDescribedFunctionTemplate() != nullptr) {
    ReportCustomDiag(uc.ctx, clang::DiagnosticsEngine::Error,
                     func->getLocation(),
                     "upper task '%0' is instantiated from a template; only a "
                     "leaf task may be, and an upper task may be an explicit "
                     "specialization (template <>) instead")
        .AddString(func->getNameAsString());
  }
  for (const clang::Stmt* child : func->getBody()->children()) {
    if (IsPassiveStmt(uc.ctx, child)) continue;
    ReportCustomDiag(uc.ctx, clang::DiagnosticsEngine::Error,
                     child->getBeginLoc(),
                     "upper task '%0' computes outside its invokes; hardware "
                     "keeps only its channel declarations and invokes, so "
                     "move this into a leaf task")
        .AddString(func->getNameAsString());
  }
}

// --- 1. Stream (FIFO) declarations. ---
// Scan the task body for `tapa::stream` / `tapa::streams` declarations and
// record each (array elements expanded to `name[i]` entries) in
// `task.streams` with its depth and VarDecl. Multi-declarator statements
// (`tapa::stream<float> a("a"), b("b");`) contribute one entry per
// declarator; the conformance corpus exercises that form.
void CollectStreamDecls(UpperTaskContext& uc, const clang::FunctionDecl* func) {
  for (const clang::Stmt* child : func->getBody()->children()) {
    const auto* decl_stmt = llvm::dyn_cast<clang::DeclStmt>(child);
    if (decl_stmt == nullptr) {
      // Any stream declared inside this non-declaration statement's
      // subtree is invisible to the collector below; diagnose it.
      DiagnoseNestedStreamDecls(uc, child);
      continue;
    }
    for (const clang::Decl* d : decl_stmt->decls()) {
      const auto* var = llvm::dyn_cast<clang::VarDecl>(d);
      if (var == nullptr) continue;
      const std::string name = var->getNameAsString();
      // A FIFO holds at least one element (a writer of an empty one blocks
      // forever), and the graph records its depth in 32 bits.
      const auto depth_of = [&](unsigned index) -> uint64_t {
        const int64_t depth = IntTemplateArg(var->getType(), index).value_or(0);
        if (depth < 1 || depth > std::numeric_limits<uint32_t>::max()) {
          ReportCustomDiag(uc.ctx, clang::DiagnosticsEngine::Error,
                           var->getLocation(),
                           "stream '%0' has depth %1; a stream holds 1 to "
                           "2^32 - 1 elements")
              << name << std::to_string(static_cast<uint64_t>(depth));
        }
        return static_cast<uint64_t>(depth);
      };
      switch (ClassifyTapaType(var->getType())) {
        case TapaKind::kStream:
          uc.task.streams[name] =
              StreamDecl{depth_of(1), var, std::nullopt, std::nullopt};
          break;
        case TapaKind::kStreams: {
          const uint64_t depth = depth_of(2);
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
}

// --- 2. Invocations. ---
// Walk `task_obj`'s `tapa::task::invoke(...)` chain: resolve the callee task,
// push one `Instance` per vector lane, and bind each argument to its callee
// port — distributing array arguments across scalar ports by access position
// and marking each stream endpoint as it is bound.
void ParseInvocations(
    UpperTaskContext& uc, const clang::Expr* task_obj,
    llvm::function_ref<std::string(const clang::FunctionDecl*, unsigned)>
        parameter_name) {
  // The child-name mangler and the access positions, which distribute an
  // array argument (streams/mmaps) across the scalar ports it feeds, in
  // order of appearance.
  const auto mangler = CreateMangleContext(uc.ctx);
  std::map<std::string, int> istreams_pos;
  std::map<std::string, int> ostreams_pos;
  std::map<std::string, int> mmaps_pos;
  std::map<const clang::Expr*, int> seq_pos;
  uint32_t position = 0;

  for (const clang::CXXMemberCallExpr* invoke : GetInvokes(task_obj)) {
    // The graph instantiates a task by its function; a callable reached
    // any other way would silently drop out of it.
    if (invoke->getNumArgs() == 0 || InvokeCallee(invoke) == nullptr) {
      const clang::Expr* what =
          invoke->getNumArgs() == 0 ? invoke : invoke->getArg(0);
      ReportCustomDiag(uc.ctx, clang::DiagnosticsEngine::Error,
                       what->getBeginLoc(),
                       "an invoke must name its task function directly; a "
                       "function pointer, a lambda or any other callable "
                       "names no task to instantiate");
      continue;
    }
    // `invoke` forwards its arguments to the task through a function
    // reference, which applies no default argument: one per parameter.
    if (const unsigned params = InvokeCallee(invoke)->getNumParams(),
        bound = static_cast<unsigned>(InvokePortArgs(invoke).size());
        bound != params) {
      ReportCustomDiag(uc.ctx, clang::DiagnosticsEngine::Error,
                       invoke->getArg(0)->getBeginLoc(),
                       "invoke binds %0 port arguments to task '%1', which "
                       "has %2 parameters; bind every parameter once")
          << std::to_string(bound) << InvokeCallee(invoke)->getNameAsString()
          << std::to_string(params);
      continue;
    }
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
        clang::Expr::EvalResult int_eval;
        // Named integers must be language constants; a read-only runtime
        // parameter cannot be folded from its declaration alone.
        const bool evaluates_to_int =
            !op_call && !is_seq &&
            (!decl_ref || arg->isIntegerConstantExpr(uc.ctx)) &&
            arg->EvaluateAsInt(int_eval, uc.ctx);
        std::optional<llvm::APSInt> as_int;
        bool too_wide = false;
        if (evaluates_to_int) {
          if (FitsIn64Bits(int_eval)) {
            as_int = int_eval.Val.getInt();
          } else {
            too_wide = true;
          }
        }
        const bool is_int_constant = evaluates_to_int && !too_wide;

        std::string arg_name;
        // Set instead of `arg_name` when the argument is an integer constant.
        std::optional<llvm::APSInt> arg_value;
        if (too_wide) {
          ReportCustomDiag(uc.ctx, clang::DiagnosticsEngine::Error,
                           arg->getBeginLoc(),
                           "integer constant bound to a task port does not "
                           "fit the task graph's 64-bit constant form");
          continue;
        } else if (is_int_constant) {
          arg_value = *as_int;
        } else if (decl_ref != nullptr) {
          arg_name = decl_ref->getNameInfo().getAsString();
        } else if (op_call != nullptr) {
          const std::optional<std::string> element =
              ArrayElementArg(uc.ctx, op_call);
          if (!element) {
            ReportCustomDiag(uc.ctx, clang::DiagnosticsEngine::Error,
                             arg->getBeginLoc(),
                             "a channel array element bound to a task port "
                             "needs an integer constant index inside the "
                             "array; hardware binds one fixed channel");
            continue;
          }
          arg_name = *element;
        }

        if (decl_ref != nullptr || op_call != nullptr || is_int_constant ||
            is_seq) {
          if (i == 0) {
            callee = decl_ref == nullptr
                         ? nullptr
                         : llvm::dyn_cast_or_null<clang::FunctionDecl>(
                               decl_ref->getDecl()->getAsFunction());
            if (callee == nullptr) break;  // not a task reference
            task_name = TaskName(*mangler, callee);
            uc.task.instances[task_name].push_back(
                Instance{mode.step, std::nullopt, {}, position++});
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
          const std::string port = parameter_name(callee, param_idx);
          const TapaKind pk = ClassifyTapaType(param);

          std::vector<Instance>& insts = uc.task.instances[task_name];
          const uint32_t inst_index = static_cast<uint32_t>(insts.size() - 1);

          auto set_arg = [&](const std::string& a, const std::string& p,
                             TapaKind cat) {
            insts.back().args[p] = Arg::Named(a, cat);
          };
          // Constants are sized to the child scalar port. `invoke` binds its
          // arguments to `Args&&...`, so the AST never holds an implicit
          // conversion to the child's parameter type -- the frontend has to
          // ask clang for one.
          auto set_const = [&](const llvm::APSInt& v, const std::string& p) {
            const clang::QualType type = param->getType();
            const uint32_t width = BitWidth(uc.ctx, type);
            if (width > 64) {
              ReportCustomDiag(uc.ctx, clang::DiagnosticsEngine::Error,
                               arg->getBeginLoc(),
                               "integer constant bound to the %0-bit task "
                               "port '%1' does not fit the task graph's "
                               "64-bit constant form")
                  << std::to_string(width) << p;
              return;
            }
            insts.back().args[p] = Arg::Constant(
                ConvertToPortType(uc.ctx, v, type), width, TapaKind::kNotTapa);
          };

          if (pk == TapaKind::kMmap || pk == TapaKind::kAsyncMmap) {
            // Memory reaches a task through the parent's own port; a local
            // copy of the parent's mmap names no port the graph can wire.
            if (decl_ref == nullptr ||
                !llvm::isa<clang::ParmVarDecl>(decl_ref->getDecl()) ||
                decl_ref->getDecl()->getDeclContext() != uc.task.def) {
              ReportCustomDiag(uc.ctx, clang::DiagnosticsEngine::Error,
                               arg->getBeginLoc(),
                               "memory argument '%0' for task port '%1' must "
                               "name a parent task parameter")
                  << arg_name << port;
              continue;
            }
            if (decl_ref != nullptr &&
                llvm::isa<clang::ParmVarDecl>(decl_ref->getDecl()) &&
                ClassifyTapaType(decl_ref->getType()) == TapaKind::kAsyncMmap) {
              uc.MarkHandedOn(arg_name, arg->getBeginLoc());
            }
            set_arg(ArrayElement(arg_name, mmaps_pos[arg_name]++, decl_ref),
                    port, pk);
          } else if (pk == TapaKind::kIStream || pk == TapaKind::kIFrames) {
            const std::string a =
                ArrayElement(arg_name, istreams_pos[arg_name]++, decl_ref);
            uc.MarkConsumer(a, task_name, inst_index, arg->getBeginLoc());
            set_arg(a, port, pk);
          } else if (pk == TapaKind::kOStream || pk == TapaKind::kOFrames) {
            const std::string a =
                ArrayElement(arg_name, ostreams_pos[arg_name]++, decl_ref);
            uc.MarkProducer(a, task_name, inst_index, arg->getBeginLoc());
            set_arg(a, port, pk);
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
            set_const(llvm::APSInt(llvm::APInt(64, seq_pos[arg]++)), port);
          } else if (arg_value) {
            set_const(*arg_value, port);
          } else {
            // Names identify parent ports, not arbitrary scalar variables
            // whose unqualified names might collide with those ports.
            const bool names_parent_parameter =
                decl_ref &&
                llvm::isa<clang::ParmVarDecl>(decl_ref->getDecl()) &&
                decl_ref->getDecl()->getDeclContext() == uc.task.def;
            if (arg->getType()->isFloatingType() && !names_parent_parameter) {
              auto diag = ReportCustomDiag(
                  uc.ctx, clang::DiagnosticsEngine::Error, arg->getBeginLoc(),
                  "floating-point argument '%0' for task port '%1' must name "
                  "a parent task parameter; floating-point constants are "
                  "unsupported");
              diag.AddString(arg_name);
              diag.AddString(port);
              continue;
            }
            if (!CanForwardScalar(uc.ctx, arg->getType(), param->getType())) {
              auto diag = ReportCustomDiag(
                  uc.ctx, clang::DiagnosticsEngine::Error, arg->getBeginLoc(),
                  "unsupported runtime scalar conversion from '%0' to '%1' "
                  "for task port '%2'; use matching scalar types and convert "
                  "inside a leaf task");
              diag.AddString(arg->getType().getAsString());
              diag.AddString(param->getType().getAsString());
              diag.AddString(port);
              continue;
            }
            if (!names_parent_parameter) {
              auto diag = ReportCustomDiag(
                  uc.ctx, clang::DiagnosticsEngine::Error, arg->getBeginLoc(),
                  "runtime scalar argument '%0' for task port '%1' must name "
                  "a parent task parameter");
              diag.AddString(arg_name);
              diag.AddString(port);
              continue;
            }
            set_arg(arg_name, port, TapaKind::kNotTapa);  // scalar
          }
          continue;
        }

        if (const auto* str = llvm::dyn_cast<clang::StringLiteral>(arg)) {
          if (i == 1 && mode.has_name && !task_name.empty()) {
            std::string name = str->getString().str();
            if (mode.vec_length > 1) {
              name += "_" + std::to_string(i_vec);
            }
            uc.task.instances[task_name].back().name = std::move(name);
            continue;
          }
        }

        // `mmaps::operator[]` hands out a reference, so an indexed region
        // arrives here, not as an indexed stream does.
        if (const auto* op = llvm::dyn_cast<clang::CXXOperatorCallExpr>(arg);
            op != nullptr && op->getOperator() == clang::OO_Subscript &&
            ClassifyTapaType(op->getArg(0)->getType()) == TapaKind::kMmaps) {
          ReportCustomDiag(uc.ctx, clang::DiagnosticsEngine::Error,
                           arg->getBeginLoc(),
                           "a region of tapa::mmaps cannot be bound by index; "
                           "pass the whole mmaps, which hands each invocation "
                           "its next region");
          continue;
        }
        ReportCustomDiag(
            uc.ctx, clang::DiagnosticsEngine::Error, arg->getBeginLoc(),
            "cannot bind a %0 here; each invoke argument after the task must "
            "be a stream or mmap channel, a scalar naming a parent task "
            "parameter, or an integer constant")
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
    const clang::SourceLocation loc = it->second.decl->getBeginLoc();
    if (!produced && !consumed) {
      ReportCustomDiag(uc.ctx, clang::DiagnosticsEngine::Warning, loc,
                       "stream '%0' is declared but never used")
          .AddString(it->first);
      it = uc.task.streams.erase(it);
    } else {
      if (produced != consumed) {
        if (consumed) {
          ReportCustomDiag(uc.ctx, clang::DiagnosticsEngine::Error, loc,
                           "stream '%0' is consumed but never produced")
              .AddString(it->first);
        } else {
          ReportCustomDiag(uc.ctx, clang::DiagnosticsEngine::Error, loc,
                           "stream '%0' is produced but never consumed")
              .AddString(it->first);
        }
      }
      ++it;
    }
  }
}

}  // namespace

void DiagnoseStrayInvokes(clang::ASTContext& ctx,
                          const clang::FunctionDecl* func) {
  if (func == nullptr || !func->hasBody()) return;
  const std::vector<const clang::CXXMemberCallExpr*> chained =
      GetInvokes(GetTapaTaskObject(func->getBody()));
  const std::set<const clang::CXXMemberCallExpr*> lowered(chained.begin(),
                                                          chained.end());
  for (const clang::CXXMemberCallExpr* invoke : GetInvokes(func->getBody())) {
    if (lowered.count(invoke) != 0) continue;
    ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error, invoke->getExprLoc(),
                     "task '%0' invokes a task outside the first tapa::task() "
                     "statement of its body; hardware instantiates only that "
                     "statement's invokes and runs any other as a plain call, "
                     "so chain every invoke onto one tapa::task() statement")
        .AddString(func->getNameAsString());
  }
}

void ParseUpperTask(
    clang::ASTContext& ctx, TaskModel& task, bool is_top,
    llvm::function_ref<std::string(const clang::FunctionDecl*, unsigned)>
        parameter_name) {
  const clang::FunctionDecl* func = task.def;
  if (func == nullptr || !func->hasBody()) return;
  const clang::Expr* task_obj = GetTapaTaskObject(func->getBody());
  if (task_obj == nullptr) return;  // leaf task

  UpperTaskContext uc(ctx, task, is_top);
  DiagnoseUpperBody(uc, func);
  CollectStreamDecls(uc, func);
  ParseInvocations(uc, task_obj, parameter_name);
  ValidateStreams(uc);
}

}  // namespace tapa::cc
