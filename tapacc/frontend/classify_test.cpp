#include "classify.h"

#include <string>

#include "gtest/gtest.h"

#include "clang/AST/Decl.h"

#include "tapa_stub_decls.h"

namespace tapa::cc {
namespace {

TapaKind ClassifyFirstParam(const std::string& decl) {
  const ParsedProbe parsed = ParseProbe(decl);
  const clang::ParmVarDecl* param = FirstParamOf(parsed.fn);
  EXPECT_NE(param, nullptr);
  return ClassifyTapaType(param);
}

TEST(Classify, StreamInterfaces) {
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::istream<float>& s);"),
            TapaKind::kIStream);
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::ostream<float>& s);"),
            TapaKind::kOStream);
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::istreams<float, 4>& s);"),
            TapaKind::kIStreams);
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::ostreams<float, 4>& s);"),
            TapaKind::kOStreams);
}

TEST(Classify, MmapInterfaces) {
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::mmap<float> m);"),
            TapaKind::kMmap);
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::mmaps<float, 2> m);"),
            TapaKind::kMmaps);
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::async_mmap<float> m);"),
            TapaKind::kAsyncMmap);
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::hmap<float, 2, 8> m);"),
            TapaKind::kHmap);
}

TEST(Classify, ScalarAndReferencePeeling) {
  EXPECT_EQ(ClassifyFirstParam("void probe(int n);"), TapaKind::kNotTapa);
  EXPECT_EQ(ClassifyFirstParam("void probe(unsigned long long n);"),
            TapaKind::kNotTapa);
  // const-ref mmap still classifies as mmap (references/const are peeled).
  EXPECT_EQ(ClassifyFirstParam("void probe(const tapa::mmap<const float>& m);"),
            TapaKind::kMmap);
  EXPECT_EQ(
      ClassifyFirstParam("void probe(const tapa::mmap<const float>&& m);"),
      TapaKind::kMmap);
}

TEST(Classify, Markers) {
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::seq s);"), TapaKind::kSeq);
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::executable e);"),
            TapaKind::kExecutable);
}

TEST(Classify, Predicates) {
  EXPECT_TRUE(IsStreamInterface(TapaKind::kIStream));
  EXPECT_TRUE(IsInputStream(TapaKind::kIStreams));
  EXPECT_FALSE(IsStreamInterface(TapaKind::kMmap));
  EXPECT_TRUE(IsMmapInterface(TapaKind::kHmap));
  EXPECT_TRUE(IsAsyncMmap(TapaKind::kAsyncMmap));
}

TEST(Classify, Cat) {
  EXPECT_STREQ(TapaKindCat(TapaKind::kIStream), "istream");
  EXPECT_STREQ(TapaKindCat(TapaKind::kMmaps), "mmap");
  EXPECT_STREQ(TapaKindCat(TapaKind::kAsyncMmap), "async_mmap");
  EXPECT_STREQ(TapaKindCat(TapaKind::kHmap), "mmap");
  EXPECT_STREQ(TapaKindCat(TapaKind::kNotTapa), "scalar");
}

}  // namespace
}  // namespace tapa::cc
