//! The [`ArtifactManifest`]: `generate_rtl`'s complete output type.

use std::collections::{BTreeMap, BTreeSet};

use crate::rtl_state::TopologyWithRtl;
use crate::support_assets::VerilogAssets;

/// The complete file set `generate_rtl` produces, keyed by the relative
/// path the caller writes each file to.
///
/// This is the codegen charter's output type
/// (`docs/src/developer/architecture.md`): generated RTL and FSM files
/// under `rtl/<name>.v`, custom-RTL template shells under
/// `template/<name>.v`, and the embedded Verilog support assets under
/// `rtl/<asset>.v`. Packaging is then a copy operation: iterate
/// [`files`](Self::files) and write each entry.
#[derive(Debug)]
pub struct ArtifactManifest {
    /// Relative path (`rtl/…`, `template/…`) → file content.
    files: BTreeMap<String, String>,
    /// The subset of `files` keys holding the embedded support assets
    /// (case-invariant; everything else is design-specific).
    support_asset_paths: BTreeSet<String>,
}

impl ArtifactManifest {
    /// Assemble the manifest from the post-pipeline state plus the embedded
    /// support assets.
    ///
    /// This is the one place outside [`crate::support_assets`] that
    /// iterates the asset set: consumers must take the assets from the
    /// manifest instead of replaying the asset loop by hand.
    pub(crate) fn collect(state: &TopologyWithRtl) -> Self {
        let mut files = BTreeMap::new();
        for (name, content) in &state.generated_files {
            files.insert(format!("rtl/{name}"), content.clone());
        }
        for (name, content) in &state.template_files {
            files.insert(format!("template/{name}"), content.clone());
        }
        let mut support_asset_paths = BTreeSet::new();
        for asset_name in VerilogAssets::iter() {
            let content = VerilogAssets::get(&asset_name).expect("iterated asset exists");
            let content =
                String::from_utf8(content.data.into_owned()).expect("assets are UTF-8 Verilog");
            let relative = format!("rtl/{asset_name}");
            files.insert(relative.clone(), content);
            support_asset_paths.insert(relative);
        }
        Self {
            files,
            support_asset_paths,
        }
    }

    /// Every shipped file: relative path → content.
    #[must_use]
    pub fn files(&self) -> &BTreeMap<String, String> {
        &self.files
    }

    /// The embedded support-asset entries, `(relative path, content)`.
    /// Case-invariant; consumers pinning outputs can dedup them across
    /// designs through this slice.
    pub fn support_asset_files(&self) -> impl Iterator<Item = (&String, &String)> {
        self.files
            .iter()
            .filter(|(path, _)| self.support_asset_paths.contains(*path))
    }

    /// The design-specific entries: everything except the support assets.
    pub fn design_files(&self) -> impl Iterator<Item = (&String, &String)> {
        self.files
            .iter()
            .filter(|(path, _)| !self.support_asset_paths.contains(*path))
    }
}
