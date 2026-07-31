//! Golden-output freeze for `tapa_codegen::generate_rtl`.
//!
//! Every directory under `tapa-core/testdata/golden/<case>/` is one golden
//! case:
//!
//! ```text
//! <case>/design.json          tapa-ir `Design` (same schema as the
//!                             `testdata/topology` conformance fixtures)
//! <case>/floorplan.json       optional `FloorplanResult`; when present it is
//!                             attached exactly like the CLI floorplan flow
//! <case>/inputs/<Task>.v      one HLS Verilog fixture per HLS task, attached
//!                             to the topology like `synth`'s HLS outputs
//! <case>/expected/rtl/*.v     blessed: generated RTL only
//! <case>/expected/template/   blessed: custom-RTL templates (only when the
//! (*.v)                       case emits any)
//! <case>/PROVENANCE.md        how the inputs were produced
//! _assets/expected/rtl/*.v    blessed ONCE: the embedded support assets
//!                             (case-invariant; shared by every case)
//! ```
//!
//! The harness drives the same public generation path the CLI pack step
//! effectively uses (`tapa_cli::steps::synth::rtl_codegen`): attach every
//! HLS task's parsed module to a `TopologyWithRtl`, set the floorplan, run
//! `generate_rtl`, then collect the case's emitted file set of generated
//! RTL and template files. The embedded support assets the pack step ships
//! alongside are pinned separately under `_assets/` (`generate_rtl`
//! deliberately does not return the assets; the CLI writes them separately
//! — the known F1 drift seam — so the harness replays that write exactly
//! once instead of blessing an identical copy per case). Asset and
//! generated names are disjoint by construction (generated files are
//! `<UpperTask>.v` / `<name>_fsm.v`), so a name collision cannot mask
//! either pin. The CLI also copies the HLS input Verilog verbatim into the
//! output tree; those byte copies are not re-pinned here (the inputs
//! themselves are the pinned fixtures under `<case>/inputs/`).
//!
//! Comparison is normalized per the refactor plan's cross-cutting standard
//! (sorted relative paths, trailing whitespace trimmed per line, single
//! trailing newline) so cosmetic churn does not cost a re-bless. A mismatch
//! reports every differing file plus the first differing line per file.
//!
//! BLESS / REGENERATE: run with `TAPA_BLESS_GOLDEN=1`:
//!
//! ```sh
//! cd tapa-core && TAPA_BLESS_GOLDEN=1 cargo test -p tapa-codegen --test golden_rtl
//! ```
//!
//! In bless mode the `expected/` tree of every case is rewritten from the
//! generated output (normalized) and the test reports what it wrote instead
//! of failing. Review the resulting diff like any source change; never bless
//! to silence an unexpected diff.

use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use tapa_codegen::rtl_state::TopologyWithRtl;
use tapa_codegen::{generate_rtl, support_assets::VerilogAssets};

/// Root of all golden cases, relative to the `tapa-codegen` package.
fn golden_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("testdata")
        .join("golden")
}

/// A golden case loaded from its directory.
struct GoldenCase {
    name: String,
    dir: PathBuf,
}

/// The emitted file set for one case (or the `_assets` pin), in pack-step
/// layout: generated RTL (and shared support assets) under `rtl/`,
/// templates under `template/` (exactly where the CLI writes them).
type EmittedSet = BTreeMap<String, String>;

/// Enumerate the case directories under `testdata/golden/`, sorted by name.
fn discover_cases(root: &Path) -> Vec<GoldenCase> {
    let mut cases: Vec<GoldenCase> = fs::read_dir(root)
        .unwrap_or_else(|e| panic!("cannot list golden root {}: {e}", root.display()))
        .map(|entry| entry.expect("golden root entry").path())
        .filter(|path| path.is_dir() && path.join("design.json").is_file())
        .map(|dir| GoldenCase {
            name: dir
                .file_name()
                .expect("case dir name")
                .to_string_lossy()
                .into_owned(),
            dir,
        })
        .collect();
    cases.sort_by(|a, b| a.name.cmp(&b.name));
    assert!(
        !cases.is_empty(),
        "no golden cases found under {}",
        root.display()
    );
    cases
}

/// Normalize emitted Verilog for comparison: trim trailing whitespace from
/// every line and end the file with exactly one newline.
fn normalize(content: &str) -> String {
    let mut lines: Vec<&str> = content.split('\n').map(str::trim_end).collect();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Run the pack-step generation path for one case and return the complete
/// emitted file set keyed by relative path.
fn run_case(case: &GoldenCase) -> EmittedSet {
    let design_json = fs::read_to_string(case.dir.join("design.json"))
        .unwrap_or_else(|e| panic!("[{}] cannot read design.json: {e}", case.name));
    let design: tapa_ir::Design = serde_json::from_str(&design_json)
        .unwrap_or_else(|e| panic!("[{}] design.json does not parse: {e}", case.name));

    let state = prepare_state(case, design);
    run_rtl_generation(case, state)
}

/// Build the `TopologyWithRtl` for one case, mirroring the CLI: the synth
/// flow feeds `generate_rtl` the work-state graph as-is, while the
/// floorplan flow flattens the graph first (the planner's `FloorplanResult`
/// names flattened FIFOs/instances) and attaches the resulting plan.
fn prepare_state(case: &GoldenCase, design: tapa_ir::Design) -> TopologyWithRtl {
    let floorplan_path = case.dir.join("floorplan.json");
    if !floorplan_path.is_file() {
        return TopologyWithRtl::new(design);
    }
    let flat =
        tapa_ir::flatten(&design).unwrap_or_else(|e| panic!("[{}] flatten failed: {e}", case.name));
    let floorplan_json = fs::read_to_string(&floorplan_path)
        .unwrap_or_else(|e| panic!("[{}] cannot read floorplan.json: {e}", case.name));
    let floorplan = serde_json::from_str::<tapa_ir::FloorplanResult>(&floorplan_json)
        .unwrap_or_else(|e| panic!("[{}] floorplan.json does not parse: {e}", case.name));
    let mut state = TopologyWithRtl::new(flat);
    state.floorplan = Some(floorplan);
    state
}

/// Attach fixtures and run `generate_rtl`, then collect the emitted set.
fn run_rtl_generation(case: &GoldenCase, mut state: TopologyWithRtl) -> EmittedSet {
    // Attach the HLS input fixtures the way the CLI attaches HLS outputs.
    let inputs_dir = case.dir.join("inputs");
    let task_names: Vec<String> = state.design.tasks.keys().cloned().collect();
    for task_name in task_names {
        if state.design.tasks[&task_name].synth != tapa_ir::SynthTarget::Hls {
            continue;
        }
        let module_path = inputs_dir.join(format!("{task_name}.v"));
        assert!(
            module_path.is_file(),
            "[{}] missing HLS input fixture {} for HLS task `{task_name}`",
            case.name,
            module_path.display()
        );
        let source = fs::read_to_string(&module_path).unwrap_or_else(|e| {
            panic!("[{}] cannot read {}: {e}", case.name, module_path.display())
        });
        let module = tapa_rtl::VerilogModule::parse(&source).unwrap_or_else(|e| {
            panic!(
                "[{}] cannot parse {}: {e}",
                case.name,
                module_path.display()
            )
        });
        assert_eq!(
            module.name,
            task_name,
            "[{}] {} declares module `{}` instead of `{task_name}`",
            case.name,
            module_path.display(),
            module.name
        );
        state
            .attach_module(&task_name, module)
            .unwrap_or_else(|e| panic!("[{}] cannot attach {task_name}: {e}", case.name));
    }

    generate_rtl(&mut state).unwrap_or_else(|e| panic!("[{}] generate_rtl failed: {e}", case.name));

    // Collect the case's emitted set: generated RTL + templates. The
    // embedded support assets are pinned once via `_assets` (below).
    let mut emitted = EmittedSet::new();
    for (name, content) in &state.generated_files {
        emitted.insert(format!("rtl/{name}"), content.clone());
    }
    for (name, content) in &state.template_files {
        emitted.insert(format!("template/{name}"), content.clone());
    }
    emitted
}

/// The case-invariant emitted set of the embedded support assets, in the
/// same `rtl/` layout the pack step ships.
fn support_asset_set() -> EmittedSet {
    VerilogAssets::iter()
        .map(|name| {
            let content = VerilogAssets::get(&name).expect("iterated asset exists");
            (
                format!("rtl/{name}"),
                String::from_utf8_lossy(&content.data).into_owned(),
            )
        })
        .collect()
}

/// The shared `_assets` pseudo-case pinning the support assets once.
fn shared_assets_case(root: &Path) -> Option<GoldenCase> {
    let dir = root.join("_assets");
    dir.is_dir().then(|| GoldenCase {
        name: "_assets".to_string(),
        dir,
    })
}

/// Read the blessed tree of one case into a path-indexed map.
fn read_blessed(expected_dir: &Path, case: &GoldenCase) -> EmittedSet {
    let mut blessed = EmittedSet::new();
    let mut stack = vec![expected_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("[{}] cannot list {}: {e}", case.name, dir.display()))
        {
            let path = entry.expect("expected entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let relative = path
                    .strip_prefix(expected_dir)
                    .expect("entry lives under expected/")
                    .to_string_lossy()
                    .replace('\\', "/");
                let content = fs::read_to_string(&path).unwrap_or_else(|e| {
                    panic!("[{}] cannot read {}: {e}", case.name, path.display())
                });
                blessed.insert(relative, content);
            }
        }
    }
    blessed
}

/// Rewrite the blessed tree of one case from the emitted set.
fn bless(case: &GoldenCase, emitted: &EmittedSet) {
    let expected_dir = case.dir.join("expected");
    if expected_dir.is_dir() {
        fs::remove_dir_all(&expected_dir).unwrap_or_else(|e| {
            panic!(
                "[{}] cannot clear {}: {e}",
                case.name,
                expected_dir.display()
            )
        });
    }
    for (relative, content) in emitted {
        let path = expected_dir.join(relative);
        fs::create_dir_all(path.parent().expect("blessed file has a parent"))
            .unwrap_or_else(|e| panic!("[{}] cannot create {}: {e}", case.name, path.display()));
        fs::write(&path, normalize(content))
            .unwrap_or_else(|e| panic!("[{}] cannot bless {}: {e}", case.name, path.display()));
    }
}

/// Compare the emitted set against the blessed tree; on mismatch, return a
/// report listing every differing file and each file's first differing line.
fn compare(case: &GoldenCase, emitted: &EmittedSet) -> Option<String> {
    let expected_dir = case.dir.join("expected");
    if !expected_dir.is_dir() {
        return Some(format!(
            "[{}] missing blessed tree {}; bless it with TAPA_BLESS_GOLDEN=1",
            case.name,
            expected_dir.display()
        ));
    }
    let blessed = read_blessed(&expected_dir, case);
    let mut report = String::new();

    for name in blessed.keys() {
        if !emitted.contains_key(name) {
            writeln!(report, "  blessed-only file (no longer emitted): {name}")
                .expect("write to String");
        }
    }
    for (name, actual_raw) in emitted {
        let actual = normalize(actual_raw);
        let Some(expected_raw) = blessed.get(name) else {
            writeln!(report, "  newly emitted file (not blessed): {name}")
                .expect("write to String");
            continue;
        };
        let expected = normalize(expected_raw);
        if actual == expected {
            continue;
        }
        let expected_lines: Vec<&str> = expected.lines().collect();
        let actual_lines: Vec<&str> = actual.lines().collect();
        let first_diff = (0..expected_lines.len().max(actual_lines.len()))
            .find(|&i| {
                expected_lines.get(i).unwrap_or(&"<eof>") != actual_lines.get(i).unwrap_or(&"<eof>")
            })
            .expect("differing files have a differing line");
        writeln!(
            report,
            "  {name}: first difference at line {}\n    expected: {}\n    actual:   {}",
            first_diff + 1,
            expected_lines.get(first_diff).unwrap_or(&"<eof>"),
            actual_lines.get(first_diff).unwrap_or(&"<eof>")
        )
        .expect("write to String");
    }
    (!report.is_empty()).then_some(report)
}

#[test]
fn golden_rtl_matches_blessed() {
    let root = golden_root();
    let cases = discover_cases(&root);
    let assets_case = shared_assets_case(&root);
    let bless_mode = env::var("TAPA_BLESS_GOLDEN").is_ok_and(|value| value == "1");

    if bless_mode {
        for case in &cases {
            let emitted = run_case(case);
            bless(case, &emitted);
            println!(
                "blessed [{}]: {} files ({} bytes normalized)",
                case.name,
                emitted.len(),
                emitted.values().map(|c| normalize(c).len()).sum::<usize>()
            );
        }
        if let Some(assets) = &assets_case {
            let emitted = support_asset_set();
            bless(assets, &emitted);
            println!("blessed [_assets]: {} files", emitted.len());
        }
        println!(
            "TAPA_BLESS_GOLDEN=1: rewrote the blessed trees of {} case(s); \
             review the diff before committing",
            cases.len()
        );
        return;
    }

    let mut failures = String::new();
    for case in &cases {
        let emitted = run_case(case);
        if let Some(report) = compare(case, &emitted) {
            write!(failures, "case `{}`:\n{report}", case.name).expect("write to String");
        }
    }
    if let Some(assets) = &assets_case {
        if let Some(report) = compare(assets, &support_asset_set()) {
            write!(failures, "case `_assets`:\n{report}").expect("write to String");
        }
    }
    assert!(
        failures.is_empty(),
        "golden RTL drift detected:\n{failures}\n\
         If this change is intentional, re-bless with: TAPA_BLESS_GOLDEN=1 \\\n  \
         cargo test -p tapa-codegen --test golden_rtl"
    );
    println!(
        "golden RTL: {} case(s) match their blessed trees",
        cases.len()
    );
}
