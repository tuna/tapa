#ifndef TAPA_CODEGEN_TEST_FS_H_
#define TAPA_CODEGEN_TEST_FS_H_

#include <dirent.h>

#include <functional>
#include <map>
#include <string>

#include "gtest/gtest.h"

#include "llvm/Support/MemoryBuffer.h"

namespace tapa::cc::test_support {

// The file's bytes, or empty (with a failed expectation) when unreadable.
inline std::string ReadFile(const std::string& path) {
  const auto buffer = llvm::MemoryBuffer::getFile(path);
  EXPECT_TRUE(buffer) << path;
  return buffer ? (*buffer)->getBuffer().str() : std::string();
}

// Every file under `root`, as "<relative path> -> <bytes>" (the tree
// manifest excluded).
inline std::map<std::string, std::string> ReadTree(const std::string& root) {
  std::map<std::string, std::string> files;
  const std::function<void(const std::string&, const std::string&)> walk =
      [&](const std::string& prefix, const std::string& dir) {
        DIR* stream = opendir(dir.c_str());
        ASSERT_NE(stream, nullptr);
        while (dirent* entry = readdir(stream)) {
          const std::string name = entry->d_name;
          if (name == "." || name == "..") continue;
          const std::string path = dir + "/" + name;
          const std::string relative = prefix + name;
          if (relative == ".tapa-tree-manifest") continue;
          if (entry->d_type == DT_DIR) {
            walk(relative + "/", path);
          } else {
            files[relative] = ReadFile(path);
          }
        }
        closedir(stream);
      };
  walk("", root);
  return files;
}

}  // namespace tapa::cc::test_support

#endif  // TAPA_CODEGEN_TEST_FS_H_
