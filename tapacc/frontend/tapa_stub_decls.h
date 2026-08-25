#ifndef TAPA_FRONTEND_TAPA_STUB_DECLS_H_
#define TAPA_FRONTEND_TAPA_STUB_DECLS_H_

namespace tapa::cc {

// Minimal stand-in declarations for the TAPA types, prepended to test snippets
// so unit tests don't pull in the full tapa-lib headers. Only the qualified
// record names and template arity matter to the frontend.
inline constexpr char kTapaStubDecls[] = R"cpp(
  namespace tapa {
  template <typename T, int Depth = 2>
  struct stream {};
  template <typename T, int N, int Depth = 2>
  struct streams {};
  template <typename T>
  struct istream {
    T read();
    T peek(void*);
  };
  template <typename T>
  struct ostream {
    void write(const T&);
  };
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
  template <typename T>
  struct async_mmap {};
  template <typename T>
  struct immap {};
  template <typename T>
  struct ommap {};
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

}  // namespace tapa::cc

#endif  // TAPA_FRONTEND_TAPA_STUB_DECLS_H_
