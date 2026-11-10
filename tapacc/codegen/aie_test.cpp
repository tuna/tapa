// The AI Engine half of the frontend contract: which tasks land on the AI
// Engine (attribute, `--task-target` mapping, upper-task propagation), the
// eligibility rules, the knobs on the wire, and the task's own variant of
// the rewritten tree that the AI Engine compiler builds.

#include <map>
#include <string>

#include "gtest/gtest.h"

#include "nlohmann/json.hpp"

#include "tree_pipeline.h"

namespace tapa::cc {
namespace {

using test_support::TreePipelineRun;
using test_support::VirtualTu;

TreePipelineRun Analyze(const std::string& name, const std::string& code,
                        std::map<std::string, SynthTarget> task_targets = {}) {
  return test_support::RunTreePipeline(
      {VirtualTu{"/proj/src/" + name + ".cpp", code}}, "Top",
      testing::TempDir() + "/aie_" + name, {"-std=c++17"}, {},
      std::move(task_targets));
}

size_t CountOccurrences(const std::string& text, const std::string& needle) {
  size_t count = 0, pos = 0;
  while ((pos = text.find(needle, pos)) != std::string::npos) {
    ++count;
    ++pos;
  }
  return count;
}

bool AnyDiagContains(const TreePipelineRun& run, const std::string& needle) {
  for (const std::string& diag : run.diags) {
    if (diag.find(needle) != std::string::npos) return true;
  }
  for (const std::string& error : run.builder.errors()) {
    if (error.find(needle) != std::string::npos) return true;
  }
  return false;
}

// A PL loader feeding an AI Engine adder; `kAdd` is spliced in.
std::string Design(const std::string& add, const std::string& load = R"cpp(
  void Load(tapa::mmap<const float> mem, int n, tapa::ostream<float>& out) {
    for (int i = 0; i < n; ++i) out.write(mem[i]);
  }
)cpp") {
  return load + add + R"cpp(
    void Store(tapa::istream<float>& in, tapa::mmap<float> mem, int n) {
      for (int i = 0; i < n; ++i) mem[i] = in.read();
    }
    void Top(tapa::mmap<const float> a, tapa::mmap<float> c, int n) {
      tapa::stream<float> a_q;
      tapa::stream<float> c_q;
      tapa::task()
          .invoke(Load, a, n, a_q)
          .invoke(Add, a_q, c_q, n)
          .invoke(Store, c_q, c, n);
    }
  )cpp";
}

TEST(AieTest, LeafFrameReachesTheGraphAndItsOwnVariant) {
  const TreePipelineRun run =
      Analyze("leaf", Design(R"cpp(
                inline float Scale(float x) {
                  float k[4] = {};
                  return x * k[0];
                }
                [[tapa::target("aie")]]
                void Add([[tapa::frame(64)]] tapa::istream<float>& a,
                         tapa::ostream<float>& c, int n) {
                  float bias[8] = {};
                  [[tapa::pipeline]] for (int i = 0; i < n; ++i)
                    c.write(Scale(Scale(a.read())) + bias[i % 8]);
                }
              )cpp"));
  ASSERT_TRUE(run.ok) << (run.diags.empty() ? "" : run.diags.front());
  const nlohmann::json add = nlohmann::json::parse(run.json)["tasks"]["Add"];
  EXPECT_EQ(add["synth"], "aie");
  EXPECT_EQ(add["ports"][0]["frame"], 64);
  // The kernel's stack holds `bias`, the loop counter, and `k` of the
  // inlined `Scale`, once however often it is called.
  EXPECT_EQ(add["aie_stack_bytes"], 8 * 4 + 4 + 4 * 4);

  // The full definition takes the frame type and the chess keyword; the
  // stub every other variant sees keeps the portable signature.
  const std::string& tu = run.files.at("leaf.cpp");
  const size_t guard = tu.find("#ifdef TAPA_TASK_DEF_Add");
  const size_t stub = tu.find("#else", guard);
  ASSERT_NE(guard, std::string::npos);
  const std::string full = tu.substr(guard, stub - guard);
  const std::string other = tu.substr(stub, tu.find("#endif", stub) - stub);
  EXPECT_NE(full.find("TAPA_AIE_INLINE void Add("), std::string::npos) << full;
  EXPECT_NE(full.find("tapa::aie::frame_t<tapa::istream<float>, 64>& a"),
            std::string::npos)
      << full;
  EXPECT_NE(full.find("chess_prepare_for_pipelining c.write"),
            std::string::npos)
      << full;
  EXPECT_NE(other.find("tapa::istream<float>& a"), std::string::npos) << other;
  EXPECT_EQ(tu.find("[[tapa::frame"), std::string::npos) << "attribute lowered";
  EXPECT_EQ(full.find("#pragma HLS"), std::string::npos)
      << "no HLS interface pragmas in the AI Engine variant: " << full;
}

TEST(AieTest, AnAsyncMmapGetsAMoverPerDirectionItsBodyUses) {
  const TreePipelineRun run = Analyze("memory", R"cpp(
    [[tapa::target("aie")]]
    void Scale(tapa::async_mmap<float>& src, tapa::async_mmap<float>& dst,
               int n) {
      float tile[16];
      src.read_addr.write({0, n});
      src.read_data.read(tile, 16);
      dst.write_addr.write({0, 16});
      dst.write_data.write(tile, 16);
    }
    void Top(tapa::mmap<float> a, tapa::mmap<float> c, int n) {
      tapa::task().invoke(Scale, a, c, n);
    }
  )cpp");
  ASSERT_TRUE(run.ok) << (run.diags.empty() ? "" : run.diags.front());
  const nlohmann::json tasks = nlohmann::json::parse(run.json)["tasks"];
  EXPECT_EQ(tasks["Scale"]["ports"][0]["aie_movers"],
            nlohmann::json({{"reads", "tapa_mover_Scale_src_reads"}}));
  EXPECT_EQ(tasks["Scale"]["ports"][1]["aie_movers"],
            nlohmann::json({{"writes", "tapa_mover_Scale_dst_writes"}}));
  const nlohmann::json& writes = tasks["tapa_mover_Scale_dst_writes"];
  EXPECT_EQ(writes["synth"], tasks["Top"]["synth"]);
  // The body never reads `write_resp`, so the mover has no response port,
  // and a write request shares its channel with the data.
  ASSERT_EQ(writes["ports"].size(), 2u) << writes;
  EXPECT_EQ(writes["ports"][1]["type"], "float");

  const std::string& tu = run.files.at("memory.cpp");
  const size_t guard =
      tu.find("#ifdef TAPA_TASK_DEF_tapa_mover_Scale_src_reads");
  ASSERT_NE(guard, std::string::npos) << tu;
  const std::string mover = tu.substr(guard, tu.find("#endif", guard) - guard);
  EXPECT_NE(mover.find("tapa::internal::serve_reads(mem, requests, data);"),
            std::string::npos)
      << mover;
  EXPECT_NE(mover.find("#pragma HLS interface ap_fifo port = requests._"),
            std::string::npos)
      << mover;
}

// An AI Engine top keeps its graph's definition, and also gets the HLS stub
// of the PL kernel that carries its frame arguments through rings where
// only PL drives the graph (a VCK5000); synth picks one per platform.
TEST(AieTest, AnAieTopAlsoGetsItsPlKernelStub) {
  const TreePipelineRun run = test_support::RunTreePipeline(
      {VirtualTu{"/proj/src/pl_kernel.cpp", R"cpp(
                   void Turn([[tapa::frame(64)]] tapa::istream<float>& in,
                             [[tapa::frame(64)]] tapa::ostream<float>& out,
                             int n) {
                     for (int i = 0; i < n * 64; ++i) out.write(in.read());
                   }
                   [[tapa::target("aie")]]
                   void Top(tapa::istream<float>& rows,
                            [[tapa::ring(8)]] tapa::ostream<float>& cols,
                            int n) {
                     tapa::task().invoke(Turn, rows, cols, n);
                   }
                 )cpp"}},
      "Top", testing::TempDir() + "/aie_pl_kernel", {"-std=c++17"}, {}, {},
      SynthTarget::kXilinxVitis);
  ASSERT_TRUE(run.ok) << (run.diags.empty() ? run.json : run.diags.front());
  const nlohmann::json top = nlohmann::json::parse(run.json)["tasks"]["Top"];
  EXPECT_EQ(top["synth"], "aie");
  EXPECT_EQ(top["ports"][0]["ring"], 4);
  EXPECT_EQ(top["ports"][1]["ring"], 8);
  EXPECT_FALSE(top["ports"][2].contains("ring"));
  EXPECT_EQ(top["defines"], nlohmann::json::array({"TAPA_TASK_DEF_Top"}));
  EXPECT_EQ(top["pl_kernel_defines"],
            nlohmann::json::array({"TAPA_PL_KERNEL_Top"}));

  const std::string& tu = run.files.at("pl_kernel.cpp");
  const size_t graph = tu.find("#ifdef TAPA_TASK_DEF_Top");
  const size_t kernel = tu.find("#elif defined(TAPA_PL_KERNEL_Top)", graph);
  const size_t stub = tu.find("#else", kernel);
  ASSERT_NE(graph, std::string::npos) << tu;
  ASSERT_NE(kernel, std::string::npos) << tu;
  ASSERT_NE(stub, std::string::npos) << tu;
  const std::string aie = tu.substr(graph, kernel - graph);
  const std::string pl = tu.substr(kernel, stub - kernel);
  const std::string other = tu.substr(stub, tu.find("#endif", stub) - stub);
  EXPECT_NE(aie.find("TAPA_AIE_INLINE void Top("), std::string::npos) << aie;
  EXPECT_NE(aie.find(".invoke(Turn"), std::string::npos) << aie;
  EXPECT_NE(pl.find("extern \"C\""), std::string::npos) << pl;
  EXPECT_NE(pl.find("Top(uint64_t rows_offset"), std::string::npos) << pl;
  EXPECT_NE(pl.find("uint32_t rows_host_frames, uint32_t& rows_kernel_frames, "
                    "uint64_t& rows_ring_shape, uint32_t cols_host_frames"),
            std::string::npos)
      << pl;
  EXPECT_NE(pl.find("HLS interface s_axilite port = cols_kernel_frames"),
            std::string::npos)
      << pl;
  EXPECT_EQ(pl.find(".invoke("), std::string::npos) << pl;
  EXPECT_NE(other.find("void Top(tapa::istream<float>& rows"),
            std::string::npos)
      << other;
  EXPECT_EQ(tu.find("[[tapa::"), std::string::npos) << tu;
}

TEST(AieTest, PipelineUnrollsNestedLoops) {
  const TreePipelineRun automatic = Analyze(
      "nested_pipeline", Design(R"cpp(
        [[tapa::target("aie")]]
        void Add(tapa::istream<float>& a, tapa::ostream<float>& c, int n) {
          [[tapa::pipeline]] for (int i = 0; i < n; ++i) {
            for (int j = 0; j < 4; ++j) c.write(a.read());
          }
        }
      )cpp"));
  ASSERT_TRUE(automatic.ok)
      << (automatic.diags.empty() ? "" : automatic.diags.front());
  const std::string& automatic_tu = automatic.files.at("nested_pipeline.cpp");
  EXPECT_NE(automatic_tu.find("chess_prepare_for_pipelining"),
            std::string::npos);
  EXPECT_EQ(CountOccurrences(automatic_tu, "chess_flatten_loop"), 1);

  const TreePipelineRun explicit_unroll = Analyze(
      "nested_explicit_unroll", Design(R"cpp(
        [[tapa::target("aie")]]
        void Add(tapa::istream<float>& a, tapa::ostream<float>& c, int n) {
          [[tapa::pipeline]] for (int i = 0; i < n; ++i) {
            [[tapa::unroll]] for (int j = 0; j < 4; ++j)
              c.write(a.read());
          }
        }
      )cpp"));
  ASSERT_TRUE(explicit_unroll.ok)
      << (explicit_unroll.diags.empty() ? "" : explicit_unroll.diags.front());
  EXPECT_EQ(
      CountOccurrences(explicit_unroll.files.at("nested_explicit_unroll.cpp"),
                       "chess_flatten_loop"),
      1);

  // A function-level pipeline has no chess form: ignored, like other
  // HLS-only attributes, so a portable source still builds.
  const TreePipelineRun function_level = Analyze(
      "function_pipeline", Design(R"cpp(
        [[tapa::target("aie")]] [[tapa::pipeline]]
        void Add(tapa::istream<float>& a, tapa::ostream<float>& c, int n) {
          for (int i = 0; i < n; ++i) c.write(a.read());
        }
      )cpp"));
  ASSERT_TRUE(function_level.ok)
      << (function_level.diags.empty() ? "" : function_level.diags.front());
  EXPECT_EQ(function_level.files.at("function_pipeline.cpp")
                .find("chess_prepare_for_pipelining"),
            std::string::npos);
}

TEST(AieTest, PlOnlyAttrsAreIgnoredWithoutChangingThePlVariant) {
  const TreePipelineRun run = Analyze(
      "pl_only_attrs", Design(R"cpp(
        [[tapa::target("aie")]]
        void Add(tapa::istream<float>& a, tapa::ostream<float>& c, int n) {
          [[tapa::partition("complete")]] float bias[4] = {};
          [[tapa::pipeline]] [[tapa::dependence("c")]]
          for (int i = 0; i < n; ++i)
            c.write(a.read() + bias[i % 4]);
        }
      )cpp"));
  ASSERT_TRUE(run.ok) << (run.diags.empty() ? "" : run.diags.front());
  const TreePipelineRun pl = Analyze(
      "pl_only_attrs_hls", Design(R"cpp(
        void Add(tapa::istream<float>& a, tapa::ostream<float>& c, int n) {
          [[tapa::partition("complete")]] float bias[4] = {};
          [[tapa::pipeline]] [[tapa::dependence("c")]]
          for (int i = 0; i < n; ++i)
            c.write(a.read() + bias[i % 4]);
        }
      )cpp"),
      {{"Add", SynthTarget::kXilinxHls}});
  ASSERT_TRUE(pl.ok) << (pl.diags.empty() ? "" : pl.diags.front());
  const std::string& tu = pl.files.at("pl_only_attrs_hls.cpp");
  EXPECT_NE(tu.find("HLS array_partition"), std::string::npos) << tu;
  EXPECT_NE(tu.find("HLS dependence"), std::string::npos) << tu;
  EXPECT_EQ(tu.find("[[tapa::partition"), std::string::npos);
  EXPECT_EQ(tu.find("[[tapa::dependence"), std::string::npos);
}

TEST(AieTest, TripcountBoundsTheLoopItIsWrittenOn) {
  const TreePipelineRun run = Analyze(
      "tripcount", Design(R"cpp(
        [[tapa::target("aie")]]
        void Add(tapa::istream<float>& a, tapa::ostream<float>& c, int n) {
          [[tapa::pipeline]] [[tapa::tripcount(1, 4)]]
          for (int i = 0; i < n; ++i)
            c.write(a.read());
          [[tapa::tripcount(2, 8)]] { c.write(a.read()); }
        }
      )cpp"));
  ASSERT_TRUE(run.ok) << (run.diags.empty() ? "" : run.diags.front());
  const std::string& tu = run.files.at("tripcount.cpp");
  EXPECT_NE(tu.find("chess_loop_range(1, 4) chess_prepare_for_pipelining"),
            std::string::npos)
      << tu;
  EXPECT_EQ(CountOccurrences(tu, "chess_loop_range"), 1)
      << "a region is not a loop: " << tu;
  EXPECT_EQ(tu.find("[[tapa::tripcount"), std::string::npos) << tu;
}

TEST(AieTest, MappingWinsOverTheAttributeAndNamesARealTask) {
  const std::string add = R"cpp(
    [[tapa::target("aie")]]
    void Add([[tapa::frame(64)]] tapa::istream<float>& a,
             tapa::ostream<float>& c, int n) {
      for (int i = 0; i < n; ++i) c.write(a.read());
    }
  )cpp";
  // Mapped to PL, the same source builds there; its frame stays in the graph
  // as the channel contract every target checks.
  const TreePipelineRun pl =
      Analyze("map_hls", Design(add), {{"Add", SynthTarget::kXilinxHls}});
  ASSERT_TRUE(pl.ok) << (pl.diags.empty() ? "" : pl.diags.front());
  const nlohmann::json pl_add = nlohmann::json::parse(pl.json)["tasks"]["Add"];
  EXPECT_EQ(pl_add["synth"], "hls");
  EXPECT_EQ(pl_add["ports"][0]["frame"], 64);

  const TreePipelineRun aie = Analyze(
      "map_aie", Design(R"cpp(
        void Add(tapa::istream<float>& a, tapa::ostream<float>& c, int n) {
          for (int i = 0; i < n; ++i) c.write(a.read());
        }
      )cpp"),
      {{"Add", SynthTarget::kAie}});
  ASSERT_TRUE(aie.ok);
  EXPECT_EQ(nlohmann::json::parse(aie.json)["tasks"]["Add"]["synth"], "aie");
  // With no attribute before `void`, the guard and the inline marker share
  // an offset; the marker must stay inside the guard, or every PL task's
  // HLS compile of this file meets an undefined `TAPA_AIE_INLINE`.
  const std::string& mapped = aie.files.at("map_aie.cpp");
  EXPECT_LT(mapped.find("#ifdef TAPA_TASK_DEF_Add"),
            mapped.find("TAPA_AIE_INLINE"))
      << mapped;

  const TreePipelineRun typo =
      Analyze("map_typo", Design(add), {{"Ad", SynthTarget::kAie}});
  EXPECT_FALSE(typo.ok);
  EXPECT_TRUE(AnyDiagContains(typo, "--target maps 'Ad'"));
}

TEST(AieTest, AnUpperTaskTargetIsItsSubtreesDefault) {
  const std::string leaves = R"cpp(
    void Inc(tapa::istream<float>& a, tapa::ostream<float>& c, int n) {
      for (int i = 0; i < n; ++i) c.write(a.read() + 1);
    }
  )cpp";
  const std::string upper = R"cpp(
    [[tapa::target("aie")]]
    void Add(tapa::istream<float>& a, tapa::ostream<float>& c, int n) {
      tapa::stream<float> mid;
      tapa::task().invoke(Inc, a, mid, n).invoke(Inc, mid, c, n);
    }
  )cpp";
  const TreePipelineRun run = Analyze("upper", Design(leaves + upper));
  ASSERT_TRUE(run.ok) << (run.diags.empty() ? "" : run.diags.front());
  const nlohmann::json tasks = nlohmann::json::parse(run.json)["tasks"];
  EXPECT_EQ(tasks["Add"]["synth"], "aie");
  EXPECT_EQ(tasks["Inc"]["synth"], "aie");
  EXPECT_EQ(tasks["Load"]["synth"], "hls");

  // An explicit target wins over the inherited one.
  const TreePipelineRun pl_leaf = Analyze(
      "upper_pl_leaf", Design("[[tapa::target(\"hls\")]]" + leaves + upper));
  ASSERT_TRUE(pl_leaf.ok) << (pl_leaf.diags.empty() ? ""
                                                    : pl_leaf.diags.front());
  EXPECT_EQ(nlohmann::json::parse(pl_leaf.json)["tasks"]["Inc"]["synth"],
            "hls");

  // Inheriting the AI Engine through one invoker and the PL through
  // another leaves no single fabric to run on.
  const TreePipelineRun both = Analyze("upper_both", leaves + upper + R"cpp(
    void Top(tapa::istream<float>& a, tapa::ostream<float>& c, int n) {
      tapa::stream<float> mid;
      tapa::task().invoke(Add, a, mid, n).invoke(Inc, mid, c, n);
    }
  )cpp");
  EXPECT_FALSE(both.ok);
  EXPECT_TRUE(AnyDiagContains(both, "inherits the AI Engine from 'Add'"));

  // An AI Engine top sets the default and stays the PL kernel itself.
  const TreePipelineRun aie_top = Analyze("aie_top", leaves + R"cpp(
    [[tapa::target("hls")]]
    void Load(tapa::mmap<const float> mem, int n, tapa::ostream<float>& out) {
      for (int i = 0; i < n; ++i) out.write(mem[i]);
    }
    [[tapa::target("hls")]]
    void Store(tapa::istream<float>& in, tapa::mmap<float> mem, int n) {
      for (int i = 0; i < n; ++i) mem[i] = in.read();
    }
    [[tapa::target("aie")]]
    void Top(tapa::mmap<const float> a, tapa::mmap<float> c, int n) {
      tapa::stream<float> a_q;
      tapa::stream<float> c_q;
      tapa::task()
          .invoke(Load, a, n, a_q)
          .invoke(Inc, a_q, c_q, n)
          .invoke(Store, c_q, c, n);
    }
  )cpp");
  ASSERT_TRUE(aie_top.ok) << (aie_top.diags.empty() ? ""
                                                    : aie_top.diags.front());
  const nlohmann::json top_tasks = nlohmann::json::parse(aie_top.json)["tasks"];
  EXPECT_EQ(top_tasks["Top"]["synth"], "hls");
  EXPECT_EQ(top_tasks["Inc"]["synth"], "aie");
  EXPECT_EQ(top_tasks["Load"]["synth"], "hls");

  // With no PL task below it and no memory, it needs no PL kernel and
  // stays on the AI Engine: its stream arguments are not AXIS ports.
  const TreePipelineRun aie_only = Analyze("aie_only", leaves + R"cpp(
    [[tapa::target("aie")]]
    void Top(tapa::istream<float>& a, tapa::ostream<float>& c, int n) {
      tapa::stream<float> mid;
      tapa::task().invoke(Inc, a, mid, n).invoke(Inc, mid, c, n);
    }
  )cpp");
  ASSERT_TRUE(aie_only.ok) << (aie_only.diags.empty() ? ""
                                                      : aie_only.diags.front());
  EXPECT_EQ(nlohmann::json::parse(aie_only.json)["tasks"]["Top"]["synth"],
            "aie");
  EXPECT_EQ(aie_only.files.at("aie_only.cpp").find("qdma_axis"),
            std::string::npos);

  // PL movers serve memory, so a top taking any stays the PL kernel.
  const TreePipelineRun memory = Analyze("aie_top_memory", R"cpp(
    void Fetch(tapa::async_mmap<float>& src, int n) {
      float tile[16];
      src.read_addr.write({0, n});
      src.read_data.read(tile, 16);
    }
    [[tapa::target("aie")]]
    void Top(tapa::mmap<float> a, int n) {
      tapa::task().invoke(Fetch, a, n);
    }
  )cpp");
  ASSERT_TRUE(memory.ok) << (memory.diags.empty() ? "" : memory.diags.front());
  const nlohmann::json memory_tasks =
      nlohmann::json::parse(memory.json)["tasks"];
  EXPECT_EQ(memory_tasks["Top"]["synth"], "hls");
  EXPECT_EQ(memory_tasks["Fetch"]["synth"], "aie");
}

TEST(AieTest, IneligibleConstructsAreRejectedAtTheirSite) {
  const struct {
    const char* name;
    const char* add;
    const char* message;
  } cases[] = {
      {"close", R"cpp(
         [[tapa::target("aie")]]
         void Add(tapa::istream<float>& a, tapa::ostream<float>& c, int n) {
           for (int i = 0; i < n; ++i) c.write(a.read());
           c.close();
         })cpp",
       "'close' in AI Engine task 'Add' has no AI Engine form"},
      {"peek", R"cpp(
         [[tapa::target("aie")]]
         void Add(tapa::istream<float>& a, tapa::ostream<float>& c, int n) {
           c.write(a.peek(nullptr));
         })cpp",
       "blocking and expose no occupancy"},
      {"narrow", R"cpp(
         [[tapa::target("aie")]]
         void Add(tapa::istream<float>& a, tapa::ostream<float>& c, int n,
                  tapa::istream<short>& s) {
           for (int i = 0; i < n; ++i) c.write(a.read());
         })cpp",
       "carries 16-bit elements"},
      {"frame_on_scalar", R"cpp(
         [[tapa::target("aie")]]
         void Add(tapa::istream<float>& a, tapa::ostream<float>& c,
                  [[tapa::frame(8)]] int n) {
           for (int i = 0; i < n; ++i) c.write(a.read());
         })cpp",
       "[[tapa::frame(N)]] needs a positive N"},
  };
  for (const auto& c : cases) {
    const TreePipelineRun run = Analyze(c.name, Design(c.add));
    EXPECT_FALSE(run.ok) << c.name;
    EXPECT_TRUE(AnyDiagContains(run, c.message)) << c.name;
  }

  const TreePipelineRun top = Analyze("top", R"cpp(
    [[tapa::target("aie")]] void Top(tapa::istream<float>& a) {
      a.read();
    }
  )cpp");
  EXPECT_FALSE(top.ok);
  EXPECT_TRUE(
      AnyDiagContains(top, "is a leaf and cannot run on the AI Engine"));
}

TEST(AieTest, BulkReadsBlockWhereNonBlockingReadsAreRejected) {
  const TreePipelineRun bulk =
      Analyze("bulk", Design(R"cpp(
                [[tapa::target("aie")]]
                void Add([[tapa::frame(64)]] tapa::istream<float>& a,
                         tapa::ostream<float>& c, int n) {
                  float block[64];
                  a.read(block, 64);
                  c.write(block, 64);
                }
              )cpp"));
  EXPECT_TRUE(bulk.ok) << (bulk.diags.empty() ? "" : bulk.diags.front());

  const TreePipelineRun nonblocking = Analyze(
      "nonblocking", Design(R"cpp(
        [[tapa::target("aie")]]
        void Add(tapa::istream<float>& a, tapa::ostream<float>& c, int n) {
          c.write(a.read(nullptr));
        }
      )cpp"));
  EXPECT_FALSE(nonblocking.ok);
  EXPECT_TRUE(
      AnyDiagContains(nonblocking, "AI Engine stream reads are blocking"));
}

TEST(AieTest, MixedTemplateTargetsGetIndependentBackends) {
  const TreePipelineRun run = Analyze("mixed_template", R"cpp(
    template <typename T>
    void Pass(tapa::istream<T>& in, tapa::ostream<T>& out) {
      [[tapa::pipeline]] for (int i = 0; i < 1; ++i)
        out.write(in.read());
    }
    [[tapa::target("aie")]]
    void AieUpper(tapa::istream<float>& in, tapa::ostream<float>& out) {
      tapa::task().invoke(Pass<float>, in, out);
    }
    void SrcF(tapa::ostream<float>& out) { out.write(0.f); }
    void SinkF(tapa::istream<float>& in) { in.read(); }
    void SrcI(tapa::ostream<int>& out) { out.write(0); }
    void SinkI(tapa::istream<int>& in) { in.read(); }
    void Top() {
      tapa::stream<float> af, bf;
      tapa::stream<int> ai, bi;
      tapa::task()
          .invoke(SrcF, af)
          .invoke(AieUpper, af, bf)
          .invoke(SinkF, bf)
          .invoke(SrcI, ai)
          .invoke(Pass<int>, ai, bi)
          .invoke(SinkI, bi);
    }
  )cpp");
  ASSERT_TRUE(run.ok) << (run.diags.empty() ? "" : run.diags.front());
  const std::string& tu = run.files.at("mixed_template.cpp");
  EXPECT_NE(tu.find("#elif "), std::string::npos) << tu;
  EXPECT_NE(tu.find("TAPA_AIE_INLINE void Pass"), std::string::npos) << tu;
  EXPECT_NE(tu.find("chess_prepare_for_pipelining"), std::string::npos) << tu;
  EXPECT_NE(tu.find("_Pragma(\"HLS pipeline\")"), std::string::npos) << tu;
}

TEST(AieTest, HelpersGetTargetVariants) {
  const TreePipelineRun run = Analyze(
      "helper_pipeline", Design(R"cpp(
        void Copy(tapa::istream<float>& in, tapa::ostream<float>& out, int n) {
          [[tapa::pipeline]] for (int i = 0; i < n; ++i)
            out.write(in.read());
        }
        [[tapa::target("aie")]]
        void Add(tapa::istream<float>& in, tapa::ostream<float>& out, int n) {
          Copy(in, out, n);
        }
      )cpp"));
  ASSERT_TRUE(run.ok) << (run.diags.empty() ? "" : run.diags.front());
  const std::string& tu = run.files.at("helper_pipeline.cpp");
  EXPECT_NE(tu.find("#elif "), std::string::npos) << tu;
  EXPECT_NE(tu.find("__attribute__((noinline))"), std::string::npos) << tu;
  EXPECT_NE(tu.find("chess_prepare_for_pipelining"), std::string::npos) << tu;
}

TEST(AieTest, RecursiveUpperTasksAreRejectedInsteadOfLooping) {
  const TreePipelineRun run = Analyze("recursive_upper", R"cpp(
    void B();
    [[tapa::target("aie")]] void A() { tapa::task().invoke(B); }
    void B() { tapa::task().invoke(A); }
    void Top() { tapa::task().invoke(A); }
  )cpp");
  EXPECT_FALSE(run.ok);
  EXPECT_TRUE(AnyDiagContains(run, "task invocation cycle"));
}

TEST(AieTest, APlTaskMustNotCloseThroughHelpersAliasesOrUpperTasks) {
  const std::string add = R"cpp(
    [[tapa::target("aie")]]
    void Add(tapa::istream<float>& a, tapa::ostream<float>& c, int n) {
      for (int i = 0; i < n; ++i) c.write(a.read());
    }
  )cpp";
  const struct {
    const char* name;
    const char* load;
  } cases[] = {
      {"helper_eot", R"cpp(
         void Finish(tapa::ostream<float>& out) {
           out.close();
         }
         void Load(tapa::mmap<const float> mem, int n, tapa::ostream<float>& out) {
           for (int i = 0; i < n; ++i) out.write(mem[i]);
           Finish(out);
         })cpp"},
      {"alias_eot", R"cpp(
         void Load(tapa::mmap<const float> mem, int n,
                   tapa::ostream<float>& out) {
           for (int i = 0; i < n; ++i) out.write(mem[i]);
           auto& alias = out;
           alias.close();
         })cpp"},
      {"upper_eot", R"cpp(
         void Leaf(tapa::ostream<float>& out) { out.close(); }
         void Load(tapa::mmap<const float> mem, int n, tapa::ostream<float>& out) {
           tapa::task().invoke(Leaf, out);
         })cpp"},
  };
  for (const auto& c : cases) {
    const TreePipelineRun run = Analyze(c.name, Design(add, c.load));
    EXPECT_FALSE(run.ok) << c.name;
    EXPECT_TRUE(AnyDiagContains(run, "closes its stream")) << c.name;
  }
}

TEST(AieTest, ForwardDeclarationsCarryFramesWithoutLeaking) {
  const std::string code = Design(R"cpp(
    void Add([[tapa::frame(64)]] tapa::istream<float>& a,
             tapa::ostream<float>& c, int n);
    [[tapa::target("aie")]]
    void Add(tapa::istream<float>& a, tapa::ostream<float>& c, int n) {
      for (int i = 0; i < n; ++i) c.write(a.read());
    }
  )cpp");
  const TreePipelineRun aie = Analyze("decl_aie", code);
  ASSERT_TRUE(aie.ok) << (aie.diags.empty() ? "" : aie.diags.front());
  const nlohmann::json add = nlohmann::json::parse(aie.json)["tasks"]["Add"];
  EXPECT_EQ(add["ports"][0]["frame"], 64);
  const std::string& tu = aie.files.at("decl_aie.cpp");
  EXPECT_NE(tu.find("tapa::aie::frame_t<tapa::istream<float>, 64>& a"),
            std::string::npos)
      << tu;
  EXPECT_EQ(tu.find("[[tapa::frame"), std::string::npos) << tu;

  const TreePipelineRun pl =
      Analyze("decl_hls", code, {{"Add", SynthTarget::kXilinxHls}});
  ASSERT_TRUE(pl.ok) << (pl.diags.empty() ? "" : pl.diags.front());
  const nlohmann::json pl_add = nlohmann::json::parse(pl.json)["tasks"]["Add"];
  EXPECT_EQ(pl_add["ports"][0]["frame"], 64);
  EXPECT_EQ(pl.files.at("decl_hls.cpp").find("[[tapa::frame"),
            std::string::npos);
}

TEST(AieTest, APlTaskMustNotCloseAStreamIntoTheAiEngine) {
  const TreePipelineRun run = Analyze("eot", R"cpp(
    void Load(tapa::mmap<const float> mem, int n, tapa::ostream<float>& out) {
      for (int i = 0; i < n; ++i) out.write(mem[i]);
      out.close();
    }
    [[tapa::target("aie")]]
    void Add(tapa::istream<float>& a, tapa::ostream<float>& c, int n) {
      for (int i = 0; i < n; ++i) c.write(a.read());
    }
    void Store(tapa::istream<float>& in, tapa::mmap<float> mem, int n) {
      for (int i = 0; i < n; ++i) mem[i] = in.read();
    }
    void Top(tapa::mmap<const float> a, tapa::mmap<float> c, int n) {
      tapa::stream<float> a_q;
      tapa::stream<float> c_q;
      tapa::task()
          .invoke(Load, a, n, a_q)
          .invoke(Add, a_q, c_q, n)
          .invoke(Store, c_q, c, n);
    }
  )cpp");
  EXPECT_FALSE(run.ok);
  EXPECT_TRUE(AnyDiagContains(
      run,
      "task 'Load' closes its stream 'out', which feeds the AI Engine "
      "task 'Add' through 'a_q' in 'Top'"));
}

}  // namespace
}  // namespace tapa::cc
