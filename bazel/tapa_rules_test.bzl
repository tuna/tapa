"""Analysis-only checks for the TAPA build pipeline's cache boundaries."""

load("@bazel_skylib//lib:unittest.bzl", "analysistest", "asserts")
load("//bazel:tapa_rules.bzl", "tapa_xo")

def _has_input(action, basename):
    for input_file in action.inputs.to_list():
        if input_file.basename == basename:
            return True
    return False

def _pipeline_actions_are_content_staged_test_impl(ctx):
    env = analysistest.begin(ctx)
    target = analysistest.target_under_test(env)
    actions = target.actions
    asserts.equals(env, 3, len(actions), "analyze, synth, and pack need independent cache keys")
    if len(actions) != 3:
        return analysistest.end(env)

    analyze, synth, pack = actions
    asserts.equals(env, "TapaAnalyze", analyze.mnemonic)
    asserts.equals(env, "TapaSynth", synth.mnemonic)
    asserts.equals(env, "TapaPack", pack.mnemonic)
    asserts.true(env, _has_input(analyze, "vadd.cpp"), "analyze must read the kernel source")
    asserts.true(env, _has_input(analyze, "vadd.h"), "analyze must read kernel headers")
    asserts.true(env, _has_input(synth, "cache-fixture-xo.analysis.tapa"), "synth must consume analyzed content")
    asserts.false(env, _has_input(synth, "vadd.cpp"), "unchanged analyzed C++ must be able to reuse synth")
    asserts.false(env, _has_input(synth, "vadd.h"), "headers must affect synth through analyzed content")
    asserts.true(env, _has_input(pack, "cache-fixture-xo.pack.tapa"), "pack must consume synthesized content")
    asserts.false(env, _has_input(pack, "cache-fixture-xo.analysis.tapa"), "pack must not depend directly on analyze")
    asserts.true(env, _has_input(pack, "overlay.v"), "custom RTL belongs to pack")
    asserts.false(env, _has_input(synth, "overlay.v"), "custom RTL must not invalidate synth")
    return analysistest.end(env)

_pipeline_actions_test = analysistest.make(_pipeline_actions_are_content_staged_test_impl)

def tapa_rules_test_suite(name):
    tapa_xo(
        name = "cache-fixture-xo",
        srcs = ["//tests/apps/vadd:vadd.cpp"],
        hdrs = ["//tests/apps/vadd:vadd.h"],
        include = ["//tests/apps/vadd:."],
        tags = ["manual"],
        custom_rtl_files = ["overlay.v"],
        top_name = "VecAdd",
    )
    _pipeline_actions_test(
        name = name + "_actions_test",
        target_under_test = ":cache-fixture-xo",
    )

def _fixture_pipeline_actions_test_impl(ctx):
    env = analysistest.begin(ctx)
    target = analysistest.target_under_test(env)
    actions = target.actions
    asserts.equals(env, 3, len(actions), "analyze, fixture synth, and pack need independent cache keys")
    if len(actions) != 3:
        return analysistest.end(env)

    _analyze, synth, _pack = actions
    asserts.equals(env, "TapaSynth", synth.mnemonic)
    asserts.true(env, _has_input(synth, "Mmap2Stream.v"), "fixture synth must read the HLS-output fixtures")
    asserts.true(
        env,
        "--skip-hls-based-on-mtime" in synth.content,
        "fixture synth must route every HLS run through the mtime cache",
    )
    return analysistest.end(env)

_fixture_pipeline_actions_test = analysistest.make(_fixture_pipeline_actions_test_impl)

def tapa_fixture_rules_test(name, target_under_test):
    """Analysis-only checks for an `hls_fixtures` `tapa_xo` target.

    Args:
      name: Test target name.
      target_under_test: Label of the fixture-backed `tapa_xo` target.
    """
    _fixture_pipeline_actions_test(
        name = name,
        target_under_test = target_under_test,
    )
