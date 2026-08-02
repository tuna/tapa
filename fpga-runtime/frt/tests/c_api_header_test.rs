//! Drift guard for the C++↔Rust ABI contract: the checked-in `c_api.h` must
//! match a fresh cbindgen generation from the `frt` crate.

#[test]
fn checked_in_c_api_header_is_fresh() {
    let bindings =
        cbindgen::generate(env!("CARGO_MANIFEST_DIR")).expect("cbindgen generation failed");
    let mut out = Vec::new();
    bindings.write(&mut out);
    let generated = String::from_utf8(out).expect("cbindgen output is utf-8");
    let header = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tapa-lib/tapa/host/frt/c_api.h"
    );
    let checked_in =
        std::fs::read_to_string(header).expect("checked-in c_api.h must be staged/readable");
    assert_eq!(
        checked_in, generated,
        "tapa-lib/tapa/host/frt/c_api.h is stale; regenerate with \
         `cargo run -p frt --example gen_c_api > tapa-lib/tapa/host/frt/c_api.h`"
    );
}
