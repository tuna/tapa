//! Emit `c_api.h` for the `frt` crate via cbindgen.
//!
//! Usage: `cargo run -p frt-cbindgen` writes the header to stdout;
//! `cargo run -p frt-cbindgen -- --check <path>` exits non-zero when
//! the checked-in header at `<path>` differs from a fresh generation.

fn generate() -> String {
    let crate_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../frt");
    let bindings = cbindgen::generate(crate_dir).expect("cbindgen generation failed");
    let mut out = Vec::new();
    bindings.write(&mut out);
    String::from_utf8(out).expect("cbindgen output is utf-8")
}

fn main() {
    let generated = generate();
    let mut args = std::env::args();
    if args.nth(1).as_deref() == Some("--check") {
        let path = args.next().expect("--check requires a path");
        let checked_in = std::fs::read_to_string(&path).expect("read checked-in header");
        if checked_in != generated {
            eprintln!("{path} is stale; regenerate with `cargo run -p frt-cbindgen`");
            std::process::exit(1);
        }
        return;
    }
    print!("{generated}");
}

#[cfg(test)]
mod tests {
    /// The checked-in header must match a fresh cbindgen generation —
    /// this is the drift guard for the C++↔Rust ABI contract.
    #[test]
    fn checked_in_c_api_header_is_fresh() {
        let generated = super::generate();
        let header = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tapa-lib/tapa/host/frt/c_api.h"
        );
        let checked_in =
            std::fs::read_to_string(header).expect("checked-in c_api.h must be staged/readable");
        assert_eq!(
            checked_in, generated,
            "tapa-lib/tapa/host/frt/c_api.h is stale; regenerate with \
             `cargo run -p frt-cbindgen > tapa-lib/tapa/host/frt/c_api.h`"
        );
    }
}
