//! Emit `c_api.h` for the `frt` crate via cbindgen.
//!
//! Usage: `cargo run -p frt --example gen_c_api` writes the header to stdout;
//! `cargo run -p frt --example gen_c_api -- --check <path>` exits non-zero
//! when the checked-in header at `<path>` differs from a fresh generation.

fn main() {
    let generated = generate();
    let mut args = std::env::args();
    if args.nth(1).as_deref() == Some("--check") {
        let path = args.next().expect("--check requires a path");
        let checked_in = std::fs::read_to_string(&path).expect("read checked-in header");
        if checked_in != generated {
            eprintln!("{path} is stale; regenerate with `cargo run -p frt --example gen_c_api`");
            std::process::exit(1);
        }
        return;
    }
    print!("{generated}");
}

fn generate() -> String {
    let bindings =
        cbindgen::generate(env!("CARGO_MANIFEST_DIR")).expect("cbindgen generation failed");
    let mut out = Vec::new();
    bindings.write(&mut out);
    String::from_utf8(out).expect("cbindgen output is utf-8")
}
