//! Subcommand handlers — one module per click command in
//! `tapa/__main__.py`. Each module exposes `Args` (clap parser),
//! `name()`, and `run(args, ctx)`.

pub mod analyze;
pub mod find_clang_binary;
pub mod floorplan;
pub mod gcc;
pub mod meta;
pub mod pack;
pub mod python_bridge;
pub mod synth;
pub mod version;

/// Set of every subcommand name the chained dispatcher recognizes.
/// Order matches click registration order in `tapa/__main__.py` so
/// `--help` / argv golden corpora line up.
pub const KNOWN_SUBCOMMANDS: &[&str] = &[
    "analyze",
    "synth",
    "pack",
    "floorplan",
    "generate-floorplan",
    "compile",
    "compile-with-floorplan-dse",
    "g++",
    "version",
    "find-clang-binary",
];

pub fn is_known(token: &str) -> bool {
    KNOWN_SUBCOMMANDS.contains(&token)
}
