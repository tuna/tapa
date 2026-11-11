#ifndef TAPA_FRONTEND_TAPA_STUB_DECLS_H_
#define TAPA_FRONTEND_TAPA_STUB_DECLS_H_

#include <memory>
#include <string>
#include <vector>

#include "gtest/gtest.h"

#include "clang/AST/ASTContext.h"
#include "clang/AST/Decl.h"
#include "clang/AST/RecursiveASTVisitor.h"
#include "clang/Frontend/ASTUnit.h"
#include "clang/Tooling/Tooling.h"

namespace tapa::cc {

// Minimal stand-in declarations for the TAPA types, prepended to test snippets
// so unit tests don't pull in the full tapa-lib headers. Only the qualified
// record names and template arity matter to the frontend.
// Tests pin line numbers past these declarations, so keep them as written.
// clang-format off
inline constexpr char kTapaStubDecls[] = R"cpp(
  namespace tapa {
  template <typename T>
  struct istream {
    T read();
    T read(void*);
    template <typename U>
    void read(U*, int);
    T peek(void*);
  };
  template <typename T>
  struct ostream {
    void write(const T&);
    template <typename U>
    void write(const U*, int);
    void close();
  };
  template <typename T, int Depth = 2>
  struct stream : istream<T>, ostream<T> {};
  template <typename T, int N, int Depth = 2>
  struct streams {};
  template <typename T, int N>
  struct istreams {};
  template <typename T, int N>
  struct ostreams {};
  template <typename T>
  struct mmap {
    T& operator[](unsigned long long);
  };
  template <typename T, int N>
  struct mmaps {};
  struct burst { template <typename O, typename N> burst(O, N); }; struct request_ostream { void write(long long); void write(const burst&); };
  template <typename T> struct async_mmap { request_ostream read_addr, write_addr; istream<T> read_data; ostream<T> write_data; istream<unsigned char> write_resp; };
  template <typename T, int N, int S>
  struct hmap {};
  // The same overload set the real headers (tapa-lib/tapa/stub/task.h)
  // offer, so tests parse the same shapes users write — in particular the
  // name-only overload, whose name_size an argument-position probe would
  // misread as a vector length.
  struct task {
    template <typename Func, typename... Args>
    task& invoke(Func&& func, Args&&... args) {
      return *this;
    }
    template <int mode, typename Func, typename... Args>
    task& invoke(Func&& func, Args&&... args) {
      return *this;
    }
    template <typename Func, typename... Args, unsigned long name_size>
    task& invoke(Func&& func, const char (&name)[name_size], Args&&... args) {
      return *this;
    }
    template <int mode, typename Func, typename... Args,
              unsigned long name_size>
    task& invoke(Func&& func, const char (&name)[name_size], Args&&... args) {
      return *this;
    }
    template <int mode, int n, typename Func, typename... Args>
    task& invoke(Func&& func, Args&&... args) {
      return *this;
    }
    template <int mode, int n, typename Func, typename... Args,
              unsigned long name_size>
    task& invoke(Func&& func, const char (&name)[name_size], Args&&... args) {
      return *this;
    }
  };
  struct seq {};
  struct executable {};
  }  // namespace tapa
)cpp";
// clang-format on

// A test snippet parsed on top of the stub decls: `ast` owns the parsed code,
// so it must outlive every use of `fn` (the function named `probe`).
struct ParsedProbe {
  std::unique_ptr<clang::ASTUnit> ast;
  const clang::FunctionDecl* fn = nullptr;
};

// Parses `code` (appended to the stub decls) and finds `probe` in it.
inline ParsedProbe ParseProbe(const std::string& code) {
  class Finder : public clang::RecursiveASTVisitor<Finder> {
   public:
    const clang::FunctionDecl* found = nullptr;
    bool VisitFunctionDecl(clang::FunctionDecl* f) {
      if (f->getNameAsString() == "probe") found = f;
      return true;
    }
  };

  ParsedProbe parsed;
  parsed.ast = clang::tooling::buildASTFromCodeWithArgs(
      std::string(kTapaStubDecls) + "\n" + code + "\n",
      std::vector<std::string>{"-std=c++17"});
  EXPECT_NE(parsed.ast, nullptr);
  Finder finder;
  finder.TraverseDecl(parsed.ast->getASTContext().getTranslationUnitDecl());
  parsed.fn = finder.found;
  EXPECT_NE(parsed.fn, nullptr);
  return parsed;
}

// The first parameter of `fn`, or null when `fn` is null or takes none.
inline const clang::ParmVarDecl* FirstParamOf(const clang::FunctionDecl* fn) {
  if (fn == nullptr || fn->getNumParams() == 0) return nullptr;
  return fn->getParamDecl(0);
}

}  // namespace tapa::cc

#endif  // TAPA_FRONTEND_TAPA_STUB_DECLS_H_
