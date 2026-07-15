#ifndef TAPA_FRONTEND_TAPA_STUB_DECLS_H_
#define TAPA_FRONTEND_TAPA_STUB_DECLS_H_

namespace tapa::cc {

// Minimal stand-in declarations for the TAPA types, prepended to test snippets
// so unit tests don't pull in the full tapa-lib headers. Only the qualified
// record names and template arity matter to the frontend.
inline constexpr char kTapaStubDecls[] = R"cpp(
  namespace tapa {
  template <typename T>
  struct stream {};
  template <typename T, int N>
  struct streams {};
  template <typename T>
  struct istream {};
  template <typename T>
  struct ostream {};
  template <typename T, int N>
  struct istreams {};
  template <typename T, int N>
  struct ostreams {};
  template <typename T>
  struct mmap {};
  template <typename T, int N>
  struct mmaps {};
  template <typename T>
  struct async_mmap {};
  template <typename T>
  struct immap {};
  template <typename T>
  struct ommap {};
  template <typename T, int N, int S>
  struct hmap {};
  struct task {
    template <typename Func, typename... Args>
    task& invoke(Func&& func, Args&&... args) {
      return *this;
    }
  };
  struct seq {};
  struct executable {};
  }  // namespace tapa
)cpp";

}  // namespace tapa::cc

#endif  // TAPA_FRONTEND_TAPA_STUB_DECLS_H_
