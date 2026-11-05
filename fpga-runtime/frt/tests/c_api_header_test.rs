//! Drift guard for the C++↔Rust ABI contract: the checked-in `c_api.h` must
//! match a fresh cbindgen generation from the `frt` crate.

#[path = "../examples/gen_c_api.rs"]
#[expect(
    dead_code,
    reason = "the example's entry point is unused here; the test reuses generate()"
)]
mod gen_c_api;

#[test]
fn checked_in_c_api_header_is_fresh() {
    let generated = gen_c_api::generate();
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
