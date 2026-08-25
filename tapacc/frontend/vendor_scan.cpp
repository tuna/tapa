#include "vendor_scan.h"

#include <string>

#include "clang/AST/ASTContext.h"
#include "clang/AST/Decl.h"
#include "clang/AST/Expr.h"
#include "clang/AST/RecursiveASTVisitor.h"
#include "clang/Basic/Diagnostic.h"
#include "clang/Basic/SourceLocation.h"
#include "clang/Basic/SourceManager.h"
#include "clang/Lex/PPCallbacks.h"
#include "clang/Lex/Pragma.h"
#include "clang/Lex/Preprocessor.h"
#include "llvm/ADT/StringRef.h"
#include "llvm/ADT/StringSwitch.h"
#include "llvm/ADT/Twine.h"

namespace tapa::cc {
namespace {

// The vendor-header scan: an ap_*/hls_* include is the root of every vendor
// type or stream usage (none is usable without its header).
// Matches the complete file NAME (a path-component boundary), so
// <myap_int.h> does not match ap_int.h.
const char* HeaderSuggestion(llvm::StringRef included) {
  const llvm::StringRef name =
      included.substr(included.rfind('/') + 1);  // npos+1 == 0: whole string
  return llvm::StringSwitch<const char*>(name)
      .Case("ap_int.h", "tapa::u<W>/tapa::i<W> from <tapa.h>")
      .Case("ap_fixed.h", "no TAPA equivalent yet")
      .Case("ap_utils.h", "tapa::wait() from <tapa.h>")
      .Case("hls_stream.h", "tapa::stream<T, depth> from <tapa.h>")
      .Case("hls_vector.h", "tapa::vec_t<T, N> from <tapa.h>")
      .Default(nullptr);
}

// The vendor-pragma scan: `#pragma HLS <name>` -> the portable replacement.
const char* PragmaSuggestion(llvm::StringRef name) {
  return llvm::StringSwitch<const char*>(name)
      .CaseLower("pipeline", "[[tapa::pipeline(II)]]")
      .CaseLower("unroll", "[[tapa::unroll(factor)]]")
      .CaseLower("loop_tripcount", "[[tapa::tripcount(min, max)]]")
      .CaseLower("loop_flatten", "[[tapa::flatten]]")
      .CaseLower("latency", "[[tapa::latency(min, max)]]")
      .CaseLower("dependence", "[[tapa::dependence(variable)]]")
      .CaseLower("expression_balance", "[[tapa::balance]]")
      .CaseLower("array_partition", "[[tapa::partition(type, factor, dim)]]")
      .CaseLower("bind_storage", "[[tapa::storage(type, impl, latency)]]")
      .CaseLower("resource",
                 "[[tapa::storage(...)]] (resource is the legacy spelling)")
      .CaseLower("aggregate", "[[tapa::aggregate]]")
      .CaseLower("array_map", "[[tapa::array_map(instance, offset, orient)]]")
      .CaseLower("bind_op", "[[tapa::bind_op(op, impl, latency)]]")
      .CaseLower("inline",
                 "the C++ `inline` keyword (tapacc emits the pragma from it)")
      .CaseLower("stream", "the tapa::stream<T, depth> template argument")
      .CaseLower("interface",
                 "nothing; tapacc synthesizes top-interface pragmas")
      .CaseLower("dataflow", "nothing; tapa tasks are the dataflow model")
      .Default(nullptr);
}

// Refine a suggestion once the rest of the pragma is known. `rest` holds the
// pragma's identifier tokens, lowercased and space-separated.
const char* QualifiedSuggestion(llvm::StringRef name, llvm::StringRef rest,
                                const char* fallback) {
  auto has = [&](llvm::StringRef word) {
    return rest.contains((word + " ").str());
  };
  if (name.equals_insensitive("pipeline")) {
    // `off` disables pipelining, spelled the way flatten(false) is.
    if (has("off")) return "[[tapa::pipeline(false)]]";
    // Vitis deprecated `enable_flush` in favour of `style = flp`, which the
    // attribute carries as its style argument.
    if (has("enable_flush")) return "[[tapa::pipeline(II, \"flp\")]]";
  }
  if (name.equals_insensitive("stream") && has("off")) {
    return "no TAPA equivalent yet; keep the vendor pragma";
  }
  if (name.equals_insensitive("dataflow")) {
    // Between plain calls inside one task, tapa's task graph cannot express
    // it: concurrency lives at invoke sites.
    return "tapa::task().invoke(...) for concurrency between tasks; no TAPA "
           "equivalent for intra-task dataflow";
  }
  return fallback;
}

void ReportVendorUse(clang::DiagnosticsEngine& diags, clang::SourceLocation loc,
                     llvm::StringRef construct, llvm::StringRef suggestion) {
  const unsigned remark = diags.getCustomDiagID(
      clang::DiagnosticsEngine::Remark,
      "'%0' is vendor-specific; the portable TAPA form is %1");
  diags.Report(loc, remark) << construct << suggestion;
}

class VendorIncludeScan : public clang::PPCallbacks {
 public:
  explicit VendorIncludeScan(clang::Preprocessor& pp) : pp_(pp) {}

  void InclusionDirective(clang::SourceLocation hash_loc, const clang::Token&,
                          llvm::StringRef file_name, bool,
                          clang::CharSourceRange, clang::OptionalFileEntryRef,
                          llvm::StringRef, llvm::StringRef,
                          const clang::Module*, bool,
                          clang::SrcMgr::CharacteristicKind) override {
    if (pp_.getSourceManager().isInSystemHeader(hash_loc)) return;
    if (const char* suggestion = HeaderSuggestion(file_name)) {
      ReportVendorUse(pp_.getDiagnostics(), hash_loc,
                      (llvm::Twine("<") + file_name + ">").str(), suggestion);
    }
  }

 private:
  clang::Preprocessor& pp_;
};

// Registered at top level under the name "HLS": it intercepts every
// `#pragma HLS <name>` and reads <name> itself.
class VendorPragmaScan : public clang::PragmaHandler {
 public:
  VendorPragmaScan() : PragmaHandler("HLS") {}

  void HandlePragma(clang::Preprocessor& pp, clang::PragmaIntroducer,
                    clang::Token& tok) override {
    // The handler receives its registered "HLS" token; read the pragma name.
    pp.LexUnexpandedToken(tok);
    // `inline` lexes as a keyword token, not an identifier: accept any
    // token carrying an IdentifierInfo.
    if (tok.getIdentifierInfo() == nullptr) return;
    const llvm::StringRef name = tok.getIdentifierInfo()->getName();
    const char* suggestion = PragmaSuggestion(name);
    if (suggestion == nullptr) return;  // vendor pragma with no mapping yet
    if (pp.getSourceManager().isInSystemHeader(tok.getLocation())) return;
    const clang::SourceLocation loc = tok.getLocation();

    // Some qualifiers change what the pragma means, and the attribute set
    // cannot express them. Naming an attribute anyway sends the reader to a
    // form that will not do what they wrote, so read the rest of the line.
    std::string rest;
    while (true) {
      pp.LexUnexpandedToken(tok);
      if (tok.is(clang::tok::eod) || tok.is(clang::tok::eof)) break;
      if (const clang::IdentifierInfo* id = tok.getIdentifierInfo()) {
        rest += id->getName().lower();
        rest += ' ';
      }
    }
    suggestion = QualifiedSuggestion(name, rest, suggestion);
    ReportVendorUse(pp.getDiagnostics(), loc, "#pragma HLS " + name.lower(),
                    suggestion);
  }
};

}  // namespace

void AttachVendorScan(clang::Preprocessor& pp) {
  pp.addPPCallbacks(std::make_unique<VendorIncludeScan>(pp));
  pp.AddPragmaHandler(new VendorPragmaScan);
}

void ScanVendorAsts(clang::ASTContext& ctx) {
  class WaitScan : public clang::RecursiveASTVisitor<WaitScan> {
   public:
    explicit WaitScan(clang::DiagnosticsEngine& diags) : diags_(diags) {}

    bool VisitCallExpr(clang::CallExpr* call) {
      const auto* callee =
          llvm::dyn_cast_or_null<clang::FunctionDecl>(call->getCalleeDecl());
      // Vendor headers overload operators heavily; their callee names are not
      // simple identifiers and must not reach getName().
      if (callee == nullptr || !callee->getDeclName().isIdentifier())
        return true;
      const llvm::StringRef name = callee->getName();
      if ((name == "ap_wait" || name == "ap_wait_n") &&
          !diags_.getSourceManager().isInSystemHeader(call->getBeginLoc())) {
        // ap_wait_n(N) waits N cycles and has its own overload; naming the
        // nullary form would send the reader to a function that cannot
        // express what they wrote.
        ReportVendorUse(diags_, call->getBeginLoc(), name,
                        name == "ap_wait_n" ? "tapa::wait(n)" : "tapa::wait()");
      }
      return true;
    }

   private:
    clang::DiagnosticsEngine& diags_;
  };

  WaitScan scan(ctx.getDiagnostics());
  scan.TraverseAST(ctx);
}

}  // namespace tapa::cc
