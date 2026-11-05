#ifndef TAPA_CODEGEN_TEST_UTIL_H_
#define TAPA_CODEGEN_TEST_UTIL_H_

#include <string>

#include "gtest/gtest.h"

#include "llvm/Support/FileSystem.h"

namespace tapa::cc::test_support {

// Substring test, for EXPECT_TRUE(Contains(...)) readability.
inline bool Contains(const std::string& haystack, const std::string& needle) {
  return haystack.find(needle) != std::string::npos;
}

// A fresh writable directory per test: "<gtest tmp>/<suite>/<name>", wiped
// and (re)created.
inline std::string TempRoot(const std::string& suite, const std::string& name) {
  const std::string root = testing::TempDir() + "/" + suite + "/" + name;
  llvm::sys::fs::remove_directories(root);
  // Both calls report success as a null error code.
  EXPECT_FALSE(llvm::sys::fs::create_directories(root));
  return root;
}

}  // namespace tapa::cc::test_support

#endif  // TAPA_CODEGEN_TEST_UTIL_H_
