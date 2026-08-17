#include "vendor_scan.h"

#include <memory>
#include <string>
#include <vector>

#include "gtest/gtest.h"

#include "clang/AST/ASTContext.h"
#include "clang/Basic/Diagnostic.h"
#include "clang/Frontend/ASTConsumers.h"
#include "clang/Frontend/CompilerInstance.h"
#include "clang/Frontend/FrontendActions.h"
#include "clang/Tooling/Tooling.h"

namespace tapa::cc {
namespace {

// Collects formatted diagnostics instead of printing them.
class CapturingDiags : public clang::DiagnosticConsumer {
 public:
  void HandleDiagnostic(clang::DiagnosticsEngine::Level level,
                        const clang::Diagnostic& info) override {
    if (level != clang::DiagnosticsEngine::Remark) return;
    llvm::SmallVector<char, 128> msg;
    info.FormatDiagnostic(msg);
    remarks_.emplace_back(msg.data(), msg.size());
  }

  const std::vector<std::string>& remarks() const { return remarks_; }

 private:
  std::vector<std::string> remarks_;
};

// Runs the full PP-phase scan (includes + pragmas) in-process: the action
// swaps in the capturing client before parsing begins.
class ScanAction : public clang::ASTFrontendAction {
 public:
  explicit ScanAction(CapturingDiags* consumer) : consumer_(consumer) {}

  bool BeginSourceFileAction(clang::CompilerInstance& ci) override {
    ci.getDiagnostics().setClient(consumer_, /*ShouldOwnClient=*/false);
    AttachVendorScan(ci.getPreprocessor());
    return true;
  }

  std::unique_ptr<clang::ASTConsumer> CreateASTConsumer(
      clang::CompilerInstance&, llvm::StringRef) override {
    return std::make_unique<clang::ASTConsumer>();
  }

 private:
  CapturingDiags* consumer_;
};

bool Contains(const std::string& haystack, const std::string& needle) {
  return haystack.find(needle) != std::string::npos;
}

std::vector<std::string> RunScan(
    const std::string& code,
    const clang::tooling::FileContentMappings& virtual_files = {}) {
  CapturingDiags consumer;
  // The callback only fires for includes that resolve, so virtual headers
  // must be mapped.
  const bool ok = clang::tooling::runToolOnCodeWithArgs(
      std::make_unique<ScanAction>(&consumer), code,
      std::vector<std::string>{"-std=c++17"}, "t.cpp", "t",
      std::make_shared<clang::PCHContainerOperations>(), virtual_files);
  EXPECT_TRUE(ok);
  return consumer.remarks();
}

TEST(VendorScan, KeywordPragmaNamesAreSuggested) {
  // `inline` lexes as a keyword token, not an identifier.
  const auto remarks = RunScan(R"cpp(
    inline void F();
    void G() {
#pragma HLS inline off
      G();
    }
  )cpp");
  ASSERT_EQ(remarks.size(), 1);
  EXPECT_TRUE(Contains(remarks[0], "'#pragma HLS inline' is vendor-specific"));
  EXPECT_TRUE(Contains(remarks[0], "the C++ `inline` keyword"));
}

TEST(VendorScan, VendorHeaderMatchRequiresBasename) {
  // myap_int.h must not trip the ap_int.h suggestion.
  const auto remarks = RunScan(
      "#include <myap_int.h>\nint x;\n",
      clang::tooling::FileContentMappings{{"myap_int.h", "using V = int;\n"}});
  ASSERT_TRUE(remarks.empty());
}

TEST(VendorScan, PragmasGetSuggestions) {
  const auto remarks = RunScan(R"cpp(
    void F(int n) {
      float acc[16];
#pragma HLS pipeline II = 1
#pragma HLS array_partition variable = acc cyclic factor = 4
#pragma HLS some_unmapped_pragma
      for (int i = 0; i < n; ++i) acc[0] = n;
    }
  )cpp");
  ASSERT_EQ(remarks.size(), 3);
  EXPECT_TRUE(
      Contains(remarks[0], "'#pragma HLS pipeline' is vendor-specific"));
  EXPECT_TRUE(Contains(remarks[0], "[[tapa::pipeline(II)]]"));
  EXPECT_TRUE(Contains(remarks[1], "'#pragma HLS array_partition'"));
  EXPECT_TRUE(Contains(remarks[1], "[[tapa::partition(type, factor, dim)]]"));
  // An unmapped pragma is named with its pass-through semantics, not
  // swallowed silently by the handler's registration.
  EXPECT_TRUE(Contains(remarks[2], "'#pragma HLS some_unmapped_pragma'"));
  EXPECT_TRUE(Contains(remarks[2], "no portable TAPA form"));
}

TEST(VendorScan, DataflowRemarkNamesTheMissingForm) {
  // The leaf dataflow pragma is kept deliberately (docs rule on it); its
  // remark says so rather than pointing at a portable form that does not
  // exist.
  const auto remarks = RunScan(R"cpp(
    void F() {
#pragma HLS dataflow
    }
  )cpp");
  ASSERT_EQ(remarks.size(), 1);
  EXPECT_TRUE(Contains(remarks[0], "no TAPA equivalent"));
}

TEST(VendorScan, VendorHeaderIncludeGetsSuggestion) {
  // The scan matches by header NAME; the mapped content is irrelevant.
  const auto remarks = RunScan(
      "#include <ap_int.h>\nint x;\n",
      clang::tooling::FileContentMappings{{"ap_int.h", "using V = int;\n"}});
  ASSERT_EQ(remarks.size(), 1);
  EXPECT_TRUE(Contains(remarks[0], "'<ap_int.h>' is vendor-specific"));
  EXPECT_TRUE(Contains(remarks[0], "tapa::u<W>/tapa::i<W>"));
}

TEST(VendorScan, ApWaitCallsGetSuggestion) {
  // The AST scan runs on a built AST with the capturing consumer attached.
  CapturingDiags consumer;
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      R"cpp(
        void ap_wait();
        void F(int n) {
          if (n > 1) ap_wait();
          n = f(n);
        }
        int f(int x) { return x + 1; }
      )cpp",
      std::vector<std::string>{"-std=c++17"}, "t.cpp", "t",
      std::make_shared<clang::PCHContainerOperations>(),
      clang::tooling::getClangStripDependencyFileAdjuster(),
      clang::tooling::FileContentMappings(), &consumer);
  ASSERT_NE(ast, nullptr);
  ScanVendorAsts(ast->getASTContext());
  ASSERT_EQ(consumer.remarks().size(), 1);
  EXPECT_TRUE(Contains(consumer.remarks()[0], "'ap_wait' is vendor-specific"));
  EXPECT_TRUE(Contains(consumer.remarks()[0], "tapa::wait()"));
}

}  // namespace
}  // namespace tapa::cc
