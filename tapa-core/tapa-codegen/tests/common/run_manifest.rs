//! The shared `generate_rtl` driver for the crate's behavior tests: stage
//! a fixture design exactly like the CLI synth flow does
//! (`TopologyWithRtl::new`, attach each task's parsed HLS module), run the
//! full pipeline, and return the complete `ArtifactManifest` — the codegen
//! charter's output type (`docs/src/developer/architecture.md`).
//!
//! Tests assert against the manifest (`rtl/<name>.v` and
//! `template/<name>.v` entries) instead of re-deriving the shipped set
//! from the post-pipeline `state` maps — `lib.rs` keeps those maps
//! "public for now" and directs consumers to the manifest. Fixture states
//! that need adjustment between construction and generation (a floorplan,
//! a mutated module) run through [`generate_manifest`] directly.

use tapa_codegen::rtl_state::TopologyWithRtl;
use tapa_codegen::{generate_rtl, ArtifactManifest};
use tapa_ir::Design;

use super::{attach_basic_modules, parse_module};

/// Run the full generation path for a fixture design and return its
/// [`ArtifactManifest`]: build the topology, attach `basic_modules` (the
/// standard `ap_clk`/`ap_rst_n`-only module) and `custom_modules`
/// (`(task name, module source)` pairs, parsed like HLS output), then run
/// `generate_rtl`.
///
/// Panics when fixture parsing, attachment, or generation fails — every
/// caller is a success-path behavior test; error-path tests drive
/// `generate_rtl` themselves.
pub fn run_manifest(
    design: Design,
    basic_modules: &[&str],
    custom_modules: &[(&str, &str)],
) -> ArtifactManifest {
    let mut state = TopologyWithRtl::new(design);
    attach_basic_modules(&mut state, basic_modules);
    for (task_name, source) in custom_modules {
        state
            .attach_module(task_name, parse_module(source))
            .unwrap();
    }
    generate_manifest(&mut state)
}

/// Run `generate_rtl` on a pre-configured state (floorplan attached,
/// module map adjusted, …) and return its complete [`ArtifactManifest`].
pub fn generate_manifest(state: &mut TopologyWithRtl) -> ArtifactManifest {
    generate_rtl(state).expect("generate_rtl should succeed for this fixture")
}

/// The content of the manifest's generated file `rtl/<name>` — the
/// manifest equivalent of indexing `state.generated_files[name]`, with
/// the design-specific file set listed in the failure message.
pub fn rtl_file<'m>(manifest: &'m ArtifactManifest, name: &str) -> &'m String {
    let path = format!("rtl/{name}");
    manifest.files().get(&path).unwrap_or_else(|| {
        panic!(
            "missing generated file {path}, got design files: {:?}",
            manifest
                .design_files()
                .map(|(path, _)| path)
                .collect::<Vec<_>>()
        )
    })
}
