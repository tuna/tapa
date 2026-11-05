//! Emit `c_api.h` for the `frt` crate via cbindgen.
//!
//! Usage: `cargo run -p frt --example gen_c_api` writes the header to stdout.
//! The drift guard `frt/tests/c_api_header_test.rs` reuses `generate`.

fn main() {
    print!("{}", generate());
}

pub fn generate() -> String {
    let bindings =
        cbindgen::generate(env!("CARGO_MANIFEST_DIR")).expect("cbindgen generation failed");
    let mut out = Vec::new();
    bindings.write(&mut out);
    String::from_utf8(out).expect("cbindgen output is utf-8")
}
