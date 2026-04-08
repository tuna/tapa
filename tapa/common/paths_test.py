from tapa.common.paths import get_tapa_ldflags


def test_get_tapa_ldflags_links_cpp_shim_before_rust_runtime() -> None:
    ldflags = get_tapa_ldflags()

    assert "-lfrt_cpp" in ldflags
    assert "-lfrt" in ldflags
    assert ldflags.index("-lfrt_cpp") < ldflags.index("-lfrt")
    assert "-lz" in ldflags
